use anyhow::Result;
use music::MusicApi;
use music::transfer::match_track;
use std::sync::Arc;

pub async fn transfer_playlist(
    src: Arc<dyn MusicApi>,
    dst: Arc<dyn MusicApi>,
    src_playlist_id: &str,
) -> Result<(String, usize, usize)> {
    let detail = src.playlist(src_playlist_id).await?;
    let new_playlist_name = detail.playlist.name;
    let total = detail.tracks.len();

    // Match everything first so a transfer with zero hits leaves no empty
    // playlist behind on the destination.
    let mut matched_ids: Vec<String> = Vec::new();
    for track in &detail.tracks {
        if !track.playable {
            continue;
        }

        let query = format!("{} {}", track.artists, track.name);
        if let Ok(candidates) = dst.search(&query).await
            && let Some(index) = match_track(track, &candidates)
            && let Some(dst_track_id) = candidates[index].id.clone()
        {
            matched_ids.push(dst_track_id);
        }
    }

    if matched_ids.is_empty() {
        return Ok((new_playlist_name, 0, total));
    }
    let dst_playlist_id = dst.create_playlist(&new_playlist_name).await?;

    let mut matched = 0;
    for dst_track_id in matched_ids {
        if dst
            .add_track_to_playlist(&dst_playlist_id, &dst_track_id)
            .await
            .is_ok()
        {
            matched += 1;
        }
    }

    Ok((new_playlist_name, matched, total))
}
