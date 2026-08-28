import tarfile

import pytest

# The tree and the counts below are asserted exactly, so the archive has to
# contain exactly these members and nothing else. Shelling out to `tar` could
# not promise that: macOS bsdtar writes an AppleDouble `._name` companion for
# every entry carrying an extended attribute, which `tar tzf` hides but
# tarfile.getmembers() returns, doubling every count. Write the archive here
# instead, so the fixture describes the input rather than the local tar.
ARCHIVE_MEMBERS = (
    ("arch", True),
    ("arch/top", False),
    ("arch/a", True),
    ("arch/a/b", True),
    ("arch/a/b/f1", False),
    ("arch/a/b/f2", False),
    ("arch/a/c", True),
    ("arch/a/c/f3", False),
    ("arch/d", True),
    ("arch/d/f4", False),
)


@pytest.fixture
def archive(tmp_path):
    target = tmp_path / "test.tar.gz"
    with tarfile.open(target, "w:gz") as handle:
        for name, is_directory in ARCHIVE_MEMBERS:
            info = tarfile.TarInfo(name)
            info.type = tarfile.DIRTYPE if is_directory else tarfile.REGTYPE
            info.mode = 0o755 if is_directory else 0o644
            handle.addfile(info)
    return target


def test_prints_sorted_tree_with_direct_entry_counts(tool, archive):
    result = tool("tardirs", str(archive))
    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert lines[0] == "Archive directory tree"
    assert lines[1] == "count = direct archive entries mapped to that directory"
    assert lines[2] == ""
    assert lines[3] == "└─ arch/  [2]"
    assert "   ├─ a/  [1]" in lines
    assert "   │  ├─ b/  [3]" in lines
    assert "   │  └─ c/  [2]" in lines
    assert "   └─ d/  [2]" in lines


def test_max_depth_limits_the_tree(tool, archive):
    result = tool("tardirs", str(archive), "2")
    assert "b/" not in result.stdout
    assert "a/" in result.stdout


def test_rejects_a_missing_archive(tool, tmp_path):
    result = tool("tardirs", str(tmp_path / "missing.tar.gz"))
    assert result.returncode == 1
    assert "File not found" in result.stderr


def test_rejects_an_unsupported_extension(tool, tmp_path):
    target = tmp_path / "archive.zip"
    target.touch()
    result = tool("tardirs", str(target))
    assert result.returncode == 1
    assert "Unsupported archive" in result.stderr
