use nom_exif::{ExifIter, MediaParser, MediaSource, TrackInfo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let ms = MediaSource::file_path(path).expect("open");
    let mut parser = MediaParser::new();

    if ms.has_exif() {
        let iter: ExifIter = parser.parse(ms).expect("parse exif");
        for entry in iter {
            println!("{:?} = {:?}", entry.tag(), entry.get_value());
        }
    } else if ms.has_track() {
        println!("-- has_track() path --");
        let info: TrackInfo = parser.parse(ms).expect("parse track");
        for (k, v) in info.iter() {
            println!("{:?} = {:?}", k, v);
        }
        if let Some(gps) = info.get_gps_info() {
            println!("GPS: {} {:?} {} {:?}", gps.latitude_ref, gps.latitude, gps.longitude_ref, gps.longitude);
        }
    } else {
        println!("no exif/track reported for this source");
    }
}
