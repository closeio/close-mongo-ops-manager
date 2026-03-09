from close_mongo_ops_manager.log_screen import LogScreen


def test_read_new_log_content_reads_full_file_initially(tmp_path):
    log_file = tmp_path / "app.log"
    log_file.write_text("line1\nline2\n")

    screen = LogScreen(str(log_file))
    content, truncated = screen._read_new_log_content()

    assert not truncated
    assert content == "line1\nline2\n"
    assert screen.last_position == len("line1\nline2\n")


def test_read_new_log_content_reads_only_appended_content(tmp_path):
    log_file = tmp_path / "app.log"
    log_file.write_text("line1\n")

    screen = LogScreen(str(log_file))
    first_read, first_truncated = screen._read_new_log_content()
    log_file.write_text("line1\nline2\n")
    second_read, second_truncated = screen._read_new_log_content()

    assert first_read == "line1\n"
    assert not first_truncated
    assert second_read == "line2\n"
    assert not second_truncated


def test_read_new_log_content_resets_when_file_truncates(tmp_path):
    log_file = tmp_path / "app.log"
    log_file.write_text("line1\nline2\n")

    screen = LogScreen(str(log_file))
    _ = screen._read_new_log_content()

    log_file.write_text("fresh\n")
    content, truncated = screen._read_new_log_content()

    assert truncated
    assert content == "fresh\n"
