//! Read-only container validation, without starting a window or decoding audio.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: inspect VIDEO")?;
    let extension = std::path::Path::new(&path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("");
    let bytes = std::fs::read(&path)?;
    let mut demuxer = hiraku_video::container::MediaDemuxer::new(bytes.into(), extension)?;
    let mut video = 0;
    let mut audio = 0;
    while let Some(chunk) = demuxer.next_chunk()? {
        match chunk {
            hiraku_video::container::DemuxedChunk::Video(_) => video += 1,
            hiraku_video::container::DemuxedChunk::Audio(_) => audio += 1,
        }
    }
    println!(
        "{}x{}, {video} video packets, {audio} audio packets",
        demuxer.video_config.coded_width, demuxer.video_config.coded_height
    );
    Ok(())
}
