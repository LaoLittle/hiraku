//! Small synthetic ISO BMFF file, built in memory without external assets/tools.
use super::*;

fn atom(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = ((data.len() + 8) as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    bytes
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}
fn fixture() -> Vec<u8> {
    let mut file = atom(b"ftyp", b"isom\0\0\0\0isomavc1");
    let offset = file.len() as u32 + 8;
    file.extend(atom(b"mdat", &[0, 0, 0, 2, 0x65, 0x80]));
    let mut mvhd = words(&[0, 0, 0, 1000, 40]);
    mvhd.resize(100, 0);
    let mut moov = atom(b"mvhd", &mvhd);
    let mut tkhd = words(&[3, 0, 0, 1, 0, 40]);
    tkhd.resize(84, 0);
    let mut trak = atom(b"tkhd", &tkhd);
    let mdhd = words(&[0, 0, 0, 1000, 40, 0]);
    let mut mdia = atom(b"mdhd", &mdhd);
    let mut hdlr = words(&[0, 0]);
    hdlr.extend_from_slice(b"vide");
    hdlr.resize(25, 0);
    mdia.extend(atom(b"hdlr", &hdlr));
    let mut entry = vec![0; 78];
    entry[7] = 1;
    entry[24..26].copy_from_slice(&64u16.to_be_bytes());
    entry[26..28].copy_from_slice(&32u16.to_be_bytes());
    entry[28..32].copy_from_slice(&0x00480000u32.to_be_bytes());
    entry[32..36].copy_from_slice(&0x00480000u32.to_be_bytes());
    entry[40..42].copy_from_slice(&1u16.to_be_bytes());
    entry[74..76].copy_from_slice(&24u16.to_be_bytes());
    entry[76..78].copy_from_slice(&u16::MAX.to_be_bytes());
    entry.extend(atom(
        b"avcC",
        &[1, 66, 0, 30, 255, 225, 0, 2, 103, 1, 1, 0, 2, 104, 1],
    ));
    let mut stsd = words(&[0, 1]);
    stsd.extend(atom(b"avc1", &entry));
    let mut stbl = atom(b"stsd", &stsd);
    stbl.extend(atom(b"stts", &words(&[0, 1, 1, 40])));
    stbl.extend(atom(b"stsc", &words(&[0, 1, 1, 1, 1])));
    stbl.extend(atom(b"stsz", &words(&[0, 0, 1, 6])));
    stbl.extend(atom(b"stco", &words(&[0, 1, offset])));
    stbl.extend(atom(b"stss", &words(&[0, 1, 1])));
    mdia.extend(atom(b"minf", &atom(b"stbl", &stbl)));
    trak.extend(atom(b"mdia", &mdia));
    moov.extend(atom(b"trak", &trak));
    file.extend(atom(b"moov", &moov));
    file
}

#[test]
fn mp4_inspection_and_packet_reading_use_symphonia() {
    let bytes = fixture();
    let metadata = inspect_media(&bytes, "mp4").expect("MP4 inspection");
    assert_eq!((metadata.width, metadata.height), (64, 32));
    let mut demux = MediaDemuxer::new(bytes.into(), "mp4").expect("MP4 demux");
    assert_eq!(demux.video_config.codec.to_string(), "avc1.42001E");
    assert!(demux.audio_config.is_none());
    let Some(DemuxedChunk::Video(packet)) = demux.next_chunk().expect("packet") else {
        panic!("video packet expected")
    };
    assert_eq!(packet.0.data.as_ref(), &[0, 0, 0, 2, 0x65, 0x80]);
    assert_eq!(packet.0.timestamp, 0);
    assert_eq!(packet.0.duration, Some(40_000));
    assert!(demux.next_chunk().expect("EOF").is_none());
}
