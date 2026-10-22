"""Checks on everything the service sends, before it reaches iTerm2 (design section 20.6)."""

import os

import pytest

import bletherneuk_bridge as bb
from bridge_helpers import CHAT


@pytest.mark.parametrize("value", [CHAT, CHAT.upper()])
def test_uuid_accepted(value):
    assert bb.is_uuid(value)


@pytest.mark.parametrize(
    "value",
    [CHAT + "\n", "\n" + CHAT, CHAT + " ", "{" + CHAT + "}", CHAT[:-1], CHAT.replace("-", ""),
     "123e4567-e89b-42d3-a456-42661417400g", "", None, 42],
)
def test_uuid_refused(value):
    assert not bb.is_uuid(value)


@pytest.mark.parametrize("name", ["retry-backoff", "v1.2_fix", "a", "A" * 64, "9lives"])
def test_worktree_name_accepted(name):
    assert bb.is_worktree_name(name)


@pytest.mark.parametrize(
    "name",
    ["", "a" * 65, "has space", "semi;colon", "dollar$", "slash/name", "retry\n", "caf\xe9",
     "--dangerously-skip-permissions", "-x", ".", "..", ".hidden", None],
)
def test_worktree_name_refused(name):
    assert not bb.is_worktree_name(name)


@pytest.mark.parametrize(
    "url",
    ["http://127.0.0.1:49152/auth?code=abc123&next=/", "http://127.0.0.1:1/", "http://127.0.0.1:65535/panel"],
)
def test_page_url_accepted(url):
    assert bb.is_page_url(url)


@pytest.mark.parametrize(
    "url",
    ["https://127.0.0.1:49152/", "http://localhost:49152/", "http://127.0.0.1/", "http://127.0.0.1:49152",
     "http://127.0.0.1:0/", "http://127.0.0.1:70000/", "http://127.0.0.1:49152@evil.example/",
     "http://127.0.0.1.evil.example:49152/", "http://127.0.0.1:49152/a b", "http://127.0.0.1:49152/\n",
     "http://127.0.0.1:49152/caf\xe9", "javascript:alert(1)", "file:///etc/passwd", "", None],
)
def test_page_url_refused(url):
    assert not bb.is_page_url(url)


def test_folder_accepted(tmp_path):
    folder = tmp_path / "acme" / "shop"
    folder.mkdir(parents=True)
    assert bb.is_safe_folder(str(folder))


@pytest.mark.parametrize("name", ["shop\x1b]0;x\x07", "shop\nrm -rf ~", "shop\x7f", "shop\u202egnp", "shop\u2066x", "shop\x85"])
def test_folder_with_control_or_bidi_characters_refused(tmp_path, name):
    folder = tmp_path / name
    folder.mkdir()
    assert folder.is_dir()  # it exists, so only the characters can be the reason
    assert not bb.is_safe_folder(str(folder))


def test_folder_must_be_absolute_existing_directory_owned_by_user(tmp_path):
    a_file = tmp_path / "notes.txt"
    a_file.write_text("x")
    assert not bb.is_safe_folder("repos/shop")
    assert not bb.is_safe_folder("/Users/someone/repos/missing-folder")
    assert not bb.is_safe_folder(str(a_file))
    assert not bb.is_safe_folder("/")  # owned by root
    assert not bb.is_safe_folder(None)
    assert not bb.is_safe_folder("/tmp\x00/x")


def test_title_backslashes_are_escaped():
    title = '\\(iterm2.alert(title: "x", subtitle: "y", buttons: ["OK"]))'
    assert bb.clean_title(title) == '\\\\(iterm2.alert(title: "x", subtitle: "y", buttons: ["OK"]))'


def test_title_drops_control_and_bidi_characters_and_is_capped():
    assert bb.clean_title("fix\nretry\x1b[31m \u202ebackoff") == "fixretry[31m backoff"
    assert len(bb.clean_title("x" * 500)) == bb.MAX_TITLE


@pytest.mark.parametrize("value", [42, b"x", ["a"]])
def test_title_must_be_text(value):
    with pytest.raises(bb.Refused):
        bb.clean_title(value)


@pytest.mark.parametrize("value", [0, 3, 2**53 - 1])
def test_block_accepted(value):
    assert bb.is_block(value)


@pytest.mark.parametrize("value", [-1, 2**53, True, "3", 3.0, None])
def test_block_refused(value):
    assert not bb.is_block(value)


@pytest.mark.parametrize(
    "value, expected",
    [(12, 12), ("12", 12), (12.0, 12), ("", None), (None, None), ("abc", None), (True, None), ("\xb2", None)],
)
def test_block_read_back_from_window_variable(value, expected):
    assert bb.block_from_variable(value) == expected


@pytest.mark.parametrize(
    "version, protocol, expected",
    [("3.7.6", (1, 23), True), ("3.8", (1, 23), True), ("3.7.6beta1", (1, 23), True),
     ("3.7.5", (1, 23), False), ("3.6.9", (1, 30), False),
     ("unknown", (1, 23), True), ("unknown", (1, 22), False), ("unknown", (0, 0), False)],
)
def test_browser_needs_iterm2_3_7_6(version, protocol, expected):
    assert bb.browser_supported(version, protocol) is expected


SUFFIX = "/Library/Application Support/bletherneuk/run/blether.sock"


def test_socket_path_default():
    env = {"HOME": "/Users/someone", "TMPDIR": "/var/folders/xy/T/"}
    assert bb.socket_path(env) == "/Users/someone" + SUFFIX


def test_socket_path_honours_bletherneuk_home():
    env = {"HOME": "/Users/someone", "BLETHERNEUK_HOME": "/Users/someone/test-home", "TMPDIR": "/var/folders/xy/T/"}
    assert bb.socket_path(env) == "/Users/someone/test-home" + SUFFIX


def test_socket_path_falls_back_to_tmpdir_when_too_long():
    env = {"HOME": "/Users/" + "x" * 80, "TMPDIR": "/var/folders/xy/T/"}
    assert bb.socket_path(env) == "/var/folders/xy/T/bletherneuk/blether.sock"


def test_socket_path_limit_is_103_bytes():
    fits = "/" + "h" * (103 - len(SUFFIX) - 1)
    assert len(fits + SUFFIX) == 103
    assert bb.socket_path({"HOME": fits, "TMPDIR": "/var/folders/xy/T"}) == fits + SUFFIX
    assert bb.socket_path({"HOME": fits + "h", "TMPDIR": "/var/folders/xy/T"}) == "/var/folders/xy/T/bletherneuk/blether.sock"


def test_socket_path_counts_bytes_not_characters():
    home = "/" + "\xe9" * (103 - len(SUFFIX) - 1)  # e-acute is two bytes in UTF-8
    assert len(home + SUFFIX) == 103
    assert bb.socket_path({"HOME": home, "TMPDIR": "/var/folders/xy/T"}) == "/var/folders/xy/T/bletherneuk/blether.sock"


def test_socket_path_without_tmpdir_uses_tmp():
    assert bb.socket_path({"HOME": "/Users/" + "x" * 80}) == "/tmp/bletherneuk/blether.sock"


def test_socket_path_reads_the_environment_by_default(monkeypatch):
    monkeypatch.setenv("BLETHERNEUK_HOME", "/Users/someone")
    monkeypatch.setenv("TMPDIR", "/var/folders/xy/T/")
    assert bb.socket_path() == os.path.join("/Users/someone", SUFFIX.lstrip("/"))
