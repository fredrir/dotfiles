use super::*;

fn sample(tctl: f64, rpm: u32) -> Sample {
    Sample {
        elapsed: 5,
        tctl: Some(tctl),
        vrm: None,
        gpu: Some(40.0),
        channels: [
            (Some(37), Some(rpm)),
            (None, None),
            (None, None),
            (None, None),
        ],
        journal_new: 0,
    }
}

#[test]
fn peaks_keep_the_maximum_of_each_series() {
    let mut peaks = Peaks::default();
    peaks.fold(&sample(70.0, 900));
    peaks.fold(&sample(75.5, 850));
    assert_eq!(peaks.tctl, Some(75.5));
    assert_eq!(peaks.rpm[0], Some(900));
    assert_eq!(peaks.vrm, None);
    assert_eq!(peaks.samples, 2);
    let keys = peaks.keys();
    assert_eq!(keys[0], ("peak_tctl".to_string(), "75.5".to_string()));
    assert_eq!(keys[3], ("peak_radiator".to_string(), "900".to_string()));
    assert_eq!(keys[1].1, "n/a");
}

#[test]
fn csv_rows_follow_the_header_columns() {
    let row = csv_row("2026-09-13T20:14:03", &sample(70.0, 900));
    assert_eq!(row.split(',').count(), CSV_HEADER.split(',').count());
    assert_eq!(row, "2026-09-13T20:14:03,5,70.0,,40.0,37,900,,,,,,,0");
}
