// Initializes the integration test cluster. Idempotent.
// Run inside the container with: mongosh --nodb --quiet init.js
//
// Phase "replsets" (before the mongos are started) initiates the replica
// sets; phase "cluster" (after) adds the shards and creates the test data.

const phase = process.env.CMOM_INIT_PHASE;

function admin(port) {
  return new Mongo(`mongodb://localhost:${port}/?directConnection=true`).getDB("admin");
}

function initiate(port, config) {
  const db = admin(port);
  try {
    db.runCommand({ replSetGetStatus: 1 });
    return; // already initiated
  } catch (e) {
    // NotYetInitialized
    if (e.code !== 94) throw e;
  }
  db.runCommand({ replSetInitiate: config });
  print(`initiated ${config._id}`);
}

function waitPrimary(port) {
  const db = admin(port);
  for (let i = 0; i < 480; i++) {
    if (db.runCommand({ hello: 1 }).isWritablePrimary) return;
    sleep(500);
  }
  throw new Error(`no primary on port ${port}`);
}

function waitSecondary(port) {
  const db = admin(port);
  for (let i = 0; i < 480; i++) {
    if (db.runCommand({ hello: 1 }).secondary) return;
    sleep(500);
  }
  throw new Error(`port ${port} did not become secondary`);
}

function initReplicaSets() {
  initiate(37019, {
    _id: "configRS",
    configsvr: true,
    members: [{ _id: 0, host: "localhost:37019" }],
  });
  initiate(37021, {
    _id: "shard01",
    members: [
      { _id: 0, host: "localhost:37021", priority: 2 },
      { _id: 1, host: "localhost:37022", priority: 1 },
      { _id: 2, host: "localhost:37023", arbiterOnly: true },
    ],
  });
  initiate(37024, {
    _id: "shard02",
    members: [
      { _id: 0, host: "localhost:37024", priority: 1 },
      // Passive member: reported in hello.passives instead of hello.hosts.
      { _id: 1, host: "localhost:37025", priority: 0 },
    ],
  });
  waitPrimary(37019);
  waitPrimary(37021);
  waitPrimary(37024);
  waitSecondary(37022);
  waitSecondary(37025);
}

function initCluster() {
  const mongos = new Mongo("mongodb://localhost:37017/");
  const db = mongos.getDB("admin");
  // A cluster-wide default write concern is required to add a shard with an
  // arbiter (shard01).
  db.runCommand({ setDefaultRWConcern: 1, defaultWriteConcern: { w: 1 } });
  const shards = db.runCommand({ listShards: 1 }).shards.map((s) => s._id);
  if (!shards.includes("shard01")) {
    db.runCommand({ addShard: "shard01/localhost:37021,localhost:37022", name: "shard01" });
    print("added shard01");
  }
  if (!shards.includes("shard02")) {
    db.runCommand({ addShard: "shard02/localhost:37024,localhost:37025", name: "shard02" });
    print("added shard02");
  }
  db.runCommand({ balancerStop: 1 });

  // cmomit.load: 1000 documents, k < 500 on shard01 and k >= 500 on shard02.
  if (!mongos.getDB("config").collections.findOne({ _id: "cmomit.load" })) {
    db.runCommand({ enableSharding: "cmomit", primaryShard: "shard01" });
    db.runCommand({ shardCollection: "cmomit.load", key: { k: 1 } });
    db.runCommand({ split: "cmomit.load", middle: { k: 500 } });
    db.runCommand({
      moveChunk: "cmomit.load",
      find: { k: 500 },
      to: "shard02",
      _waitForDelete: true,
    });
    print("sharded cmomit.load");
  }
  const docs = (n) => Array.from({ length: n }, (_, k) => ({ k, pad: "x".repeat(64) }));
  const load = mongos.getDB("cmomit").load;
  if (load.countDocuments({}) !== 1000) {
    load.deleteMany({});
    load.insertMany(docs(1000), { writeConcern: { w: 2 } });
    print("inserted cmomit.load documents");
  }

  const standalone = new Mongo("mongodb://localhost:37030/").getDB("cmomit").load;
  if (standalone.countDocuments({}) !== 500) {
    standalone.deleteMany({});
    standalone.insertMany(docs(500));
    print("inserted standalone documents");
  }

  // The second mongos must know the shards too.
  const other = new Mongo("mongodb://localhost:37018/").getDB("admin");
  if (other.runCommand({ listShards: 1 }).shards.length !== 2) {
    throw new Error("mongos 37018 does not list both shards");
  }
}

if (phase === "replsets") {
  initReplicaSets();
} else if (phase === "cluster") {
  initCluster();
} else {
  throw new Error(`unknown CMOM_INIT_PHASE: ${phase}`);
}
