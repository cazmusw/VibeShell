use mosaic_shell_tools::temperatures::{self, Kind, Reading};
use std::{fs, path::PathBuf};

#[test]
fn reads_thermal_and_hwmon_without_turning_missing_data_into_zero() {
    let root =
        std::env::temp_dir().join(format!("mosaic-temperatures-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let cpu = root.join("hwmon/hwmon0");
    let gpu = root.join("hwmon/hwmon1");
    let zone = root.join("thermal/thermal_zone0");
    fs::create_dir_all(&cpu).unwrap();
    fs::create_dir_all(gpu.join("device/power")).unwrap();
    fs::create_dir_all(&zone).unwrap();
    for (path, data) in [
        (cpu.join("name"), "k10temp\n"),
        (cpu.join("temp1_input"), "0\n"),
        (cpu.join("temp1_label"), "Tctl\n"),
        (cpu.join("temp2_input"), "broken\n"),
        (cpu.join("temp3_input"), "35050\n"),
        (cpu.join("temp99_crit"), "90000\n"),
        (gpu.join("name"), "amdgpu\n"),
        (gpu.join("temp1_input"), "62000\n"),
        (gpu.join("device/power/runtime_status"), "suspended\n"),
        (zone.join("type"), "acpitz\n"),
        (zone.join("temp"), "-2500\n"),
    ] {
        fs::write(path, data).unwrap();
    }
    let data = temperatures::read_class(&root);
    assert_eq!(data.sensors.len(), 5);
    assert_eq!(data.hottest(Kind::Cpu), Some(35050));
    assert_eq!(data.hottest(Kind::Gpu), None);
    assert_eq!(data.sensors[0].label, "Tctl");
    assert_eq!(data.sensors[0].reading, Reading::Value { milli_celsius: 0 });
    assert_eq!(data.sensors[1].reading, Reading::Unavailable);
    assert_eq!(data.sensors[3].reading, Reading::Suspended);
    assert_eq!(
        data.sensors[4].reading,
        Reading::Value {
            milli_celsius: -2500
        }
    );
    fs::remove_file(cpu.join("temp3_input")).unwrap();
    fs::write(gpu.join("device/power/runtime_status"), "active\n").unwrap();
    let changed = temperatures::read_class(&root);
    assert_eq!(changed.hottest(Kind::Cpu), Some(0));
    assert_eq!(changed.hottest(Kind::Gpu), Some(62000));
    assert_eq!(changed.sensors.len(), 4);
}
