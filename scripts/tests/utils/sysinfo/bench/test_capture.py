import json

from tools.utils.sysinfo.bench import capture


def test_the_embedded_snapshot_describes_the_hardware(workstation_snapshot, monkeypatch):
    monkeypatch.setattr(capture, "detect_virtualized", lambda: False)

    described = capture.describe_snapshot(workstation_snapshot)

    assert described["cpu"]["model"] == "AMD Ryzen 7 9800X3D"
    assert described["cpu"]["cores_physical"] == 8
    assert described["memory"]["total"] == 32 * 1024**3
    assert [gpu["name"] for gpu in described["gpu"]] == [
        "AMD Radeon Graphics",
        "NVIDIA GeForce RTX 5070 Ti",
    ]
    assert [disk["name"] for disk in described["disks"]] == [
        "KINGSTON SNVS2000G",
        "WDC WD20EZRZ-00Z",
    ]


def test_no_serial_reaches_the_stored_record(workstation_snapshot, monkeypatch):
    monkeypatch.setattr(capture, "detect_virtualized", lambda: False)

    serialised = json.dumps(capture.describe_snapshot(workstation_snapshot))

    assert "PRIVATE" not in serialised
    assert "SERIAL" not in serialised.upper()


def test_virtual_disks_are_left_out(macos_snapshot, monkeypatch):
    monkeypatch.setattr(capture, "detect_virtualized", lambda: False)

    described = capture.describe_snapshot(macos_snapshot)

    assert [disk["name"] for disk in described["disks"]] == ["APPLE SSD AP1024Z Media"]


def test_the_configured_hardware_travels_with_the_run(workstation_snapshot, monkeypatch):
    monkeypatch.setattr(capture, "detect_virtualized", lambda: False)

    described = capture.describe_snapshot(workstation_snapshot)

    assert described["configured"]["cpu_cooler"] == "Noctua NH-D15"


def test_the_install_records_the_distro_and_kernel(workstation_snapshot):
    install = capture.describe_install(workstation_snapshot)

    assert install["os"] == "arch"
    assert install["kernel"] == "7.1.5-arch1-2"
    assert install["arch"] == "x86_64"
    assert install["driver"] == "610.43.03"


def test_a_machine_without_nvidia_falls_back_to_the_gpu_driver(macos_snapshot):
    install = capture.describe_install(macos_snapshot)

    assert install["os"] == "macos"
    assert install["driver"] == "com.apple.AGXG17X 351.2"
