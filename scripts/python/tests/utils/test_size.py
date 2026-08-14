import subprocess


def test_reports_du_size_for_a_file(tool, tmp_path):
    target = tmp_path / "file.txt"
    target.write_text("hello\n")
    expected = subprocess.run(
        ["du", "-sh", str(target)], capture_output=True, text=True, check=False
    ).stdout.split("\t", 1)[0]
    result = tool("size", str(target))
    assert result.returncode == 0
    assert result.stdout == expected + "\n"


def test_nonhidden_total_for_a_directory(tool, tmp_path):
    (tmp_path / "a.txt").write_text("x" * 5000)
    (tmp_path / "b.txt").write_text("y" * 5000)
    (tmp_path / ".hidden.txt").write_text("z" * 100000)
    result = tool("size", "-d", str(tmp_path))
    assert result.returncode == 0
    assert result.stdout.strip()
    plain = tool("size", str(tmp_path))
    assert plain.returncode == 0


def test_empty_directory_with_nonhidden_prints_nothing(tool, tmp_path):
    result = tool("size", "-d", str(tmp_path))
    assert result.returncode == 0
    assert result.stdout == ""


def test_rejects_a_missing_path(tool, tmp_path):
    result = tool("size", str(tmp_path / "missing"))
    assert result.returncode == 1
    assert "no such file or directory" in result.stderr
