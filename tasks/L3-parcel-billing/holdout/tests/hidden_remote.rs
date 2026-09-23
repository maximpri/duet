use parcelflow::model::Destination;
use parcelflow::zones::RemoteAreas;
use std::path::Path;

#[test]
fn both_range_ends_are_remote() {
    let areas = RemoteAreas::parse("country,from_inclusive,to_inclusive,note\nNO,9000,9990,north\n", "t").unwrap();
    assert!(areas.is_remote(&Destination::new("NO", "9000")));
    assert!(areas.is_remote(&Destination::new("NO", "9990")));
    assert!(!areas.is_remote(&Destination::new("NO", "9991")));
    assert!(!areas.is_remote(&Destination::new("NO", "8999")));
}

#[test]
fn single_code_ranges_are_remote() {
    let areas = RemoteAreas::parse("country,from_inclusive,to_inclusive,note\nDE,27498,27498,island\n", "t").unwrap();
    assert!(areas.is_remote(&Destination::new("DE", "27498")));
    assert!(!areas.is_remote(&Destination::new("DE", "27497")));
    assert!(!areas.is_remote(&Destination::new("DE", "27499")));
}

#[test]
fn the_remote_area_table_covers_its_listed_codes() {
    let areas = RemoteAreas::load(Path::new("config/remote_areas.csv")).unwrap();
    for (c, p) in [("DE", "27498"), ("DE", "18565"), ("FI", "99999"), ("NO", "9990"), ("SE", "984 99"), ("ES", "35660")]
    {
        assert!(areas.is_remote(&Destination::new(c, p)), "{c}-{p}");
    }
    assert!(!areas.is_remote(&Destination::new("DE", "27499")));
}
