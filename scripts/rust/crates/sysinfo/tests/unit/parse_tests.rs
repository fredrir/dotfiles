use super::*;

#[test]
fn prunes_the_names_the_committed_baselines_depend_on() {
    assert_eq!(
        pruned_cpu_name("AMD Ryzen 7 9800X3D 8-Core Processor"),
        "AMD Ryzen 7 9800X3D"
    );
    assert_eq!(
        pruned_cpu_name("Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz"),
        "Intel Core i7-9700K"
    );
    assert_eq!(
        pruned_cpu_name("AMD Ryzen 7 5700G with Radeon Graphics"),
        "AMD Ryzen 7 5700G"
    );
}

#[test]
fn ranks_march_levels() {
    assert_eq!(
        x86_march("avx512f avx512bw avx512cd avx512dq avx512vl avx2 bmi2 fma movbe xsave"),
        "x86_64-v4"
    );
    assert_eq!(x86_march("avx2 bmi2 fma movbe xsave sse4_2"), "x86_64-v3");
    assert_eq!(x86_march("sse4_2 popcnt ssse3 cx16"), "x86_64-v2");
    assert_eq!(x86_march("sse2"), "x86_64");
}

#[test]
fn reads_os_release_with_and_without_quotes() {
    let body = "NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n";
    assert_eq!(
        os_release_field(body, "NAME").as_deref(),
        Some("Arch Linux")
    );
    assert_eq!(os_release_field(body, "ID").as_deref(), Some("arch"));
    assert_eq!(os_release_field(body, "VERSION_ID"), None);
}

#[test]
fn reads_cpuinfo_fields() {
    let body = "processor\t: 0\nmodel name\t: AMD Ryzen 7 9800X3D 8-Core Processor\n";
    assert_eq!(
        cpuinfo_value(body, "model name").as_deref(),
        Some("AMD Ryzen 7 9800X3D 8-Core Processor")
    );
}

#[test]
fn resolves_gpu_names_from_the_id_databases() {
    let pci = "10de  NVIDIA Corporation\n\t2c05  GB203 [GeForce RTX 5070 Ti]\n\
                   \t\t1043 8a2c  Subsystem\n1002  Advanced Micro Devices\n";
    assert_eq!(
        pci_device_name_in(pci, "0x10de", "0x2c05").as_deref(),
        Some("GeForce RTX 5070 Ti")
    );
    assert_eq!(pci_device_name_in(pci, "0x10de", "9999"), None);
    let amd = "13C0,\t00,\tAMD Radeon Graphics\n164E,\tC1,\tAMD Radeon 610M\n";
    assert_eq!(
        amdgpu_name_in(amd, "0x13c0", "0x00").as_deref(),
        Some("AMD Radeon Graphics")
    );
}
