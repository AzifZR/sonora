#![allow(dead_code)]

use music::Track;

pub const MIX_LIMIT: usize = 25;

pub fn heavy_rotation(history: &[Track], limit: usize) -> Vec<Track> {
    let mut counts: Vec<(&Track, usize)> = Vec::new();
    for track in history {
        if track.id.is_none() {
            continue;
        }
        if let Some(pos) = counts.iter().position(|(t, _)| t.id == track.id) {
            counts[pos].1 += 1;
        } else {
            counts.push((track, 1));
        }
    }

    counts.sort_by_key(|b| std::cmp::Reverse(b.1));

    counts
        .into_iter()
        .take(limit)
        .map(|(t, _)| t.clone())
        .collect()
}

pub fn recent_discoveries(library: &[Track], limit: usize) -> Vec<Track> {
    let mut tracks: Vec<_> = library.iter().collect();
    tracks.sort_by(|a, b| {
        let a_time = a.added_at.unwrap_or(i64::MIN);
        let b_time = b.added_at.unwrap_or(i64::MIN);
        b_time.cmp(&a_time)
    });

    tracks.into_iter().take(limit).cloned().collect()
}

pub fn short_and_sweet(library: &[Track], limit: usize) -> Vec<Track> {
    library
        .iter()
        .filter(|t| t.duration.as_secs() < 180)
        .take(limit)
        .cloned()
        .collect()
}

pub fn forgotten_favorites(library: &[Track], history: &[Track], limit: usize) -> Vec<Track> {
    let mut tracks: Vec<_> = library
        .iter()
        .filter(|t| {
            if t.id.is_none() {
                return true;
            }
            !history.iter().any(|h| h.id == t.id)
        })
        .collect();

    tracks.sort_by(|a, b| {
        let a_time = a.added_at.unwrap_or(i64::MIN);
        let b_time = b.added_at.unwrap_or(i64::MIN);
        a_time.cmp(&b_time)
    });

    tracks.into_iter().take(limit).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use music::{ArtistRef, Track};
    use std::time::Duration;

    fn track(index: usize, id_str: Option<&str>, added: Option<i64>, secs: u64) -> Track {
        Track {
            id: id_str.map(|s| s.to_string()),
            name: format!("Track {index}"),
            playable: true,
            artists: "Artist".to_string(),
            artist_refs: vec![ArtistRef {
                name: "Artist".to_string(),
                id: None,
            }],
            album: String::new(),
            album_id: None,
            cover: None,
            duration: Duration::from_secs(secs),
            added_at: added,
            added_by: None,
            playcount: None,
            popularity: 0,
            explicit: false,
            track_number: 0,
            disc_number: 0,
            tags: Vec::new(),
            languages: Vec::new(),
            credits: Vec::new(),
        }
    }

    #[test]
    fn test_heavy_rotation() {
        let t1 = track(1, Some("id1"), None, 100);
        let t2 = track(2, Some("id2"), None, 100);
        let t3 = track(3, None, None, 100);

        let history = vec![
            t1.clone(),
            t2.clone(),
            t1.clone(),
            t3.clone(),
            t2.clone(),
            t1.clone(),
        ];

        let res = heavy_rotation(&history, 2);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].id.as_deref(), Some("id1")); // 3 plays
        assert_eq!(res[1].id.as_deref(), Some("id2")); // 2 plays

        let res_limit = heavy_rotation(&history, 1);
        assert_eq!(res_limit.len(), 1);

        assert!(heavy_rotation(&[], 5).is_empty());
    }

    #[test]
    fn test_heavy_rotation_ties() {
        let t1 = track(1, Some("id1"), None, 100);
        let t2 = track(2, Some("id2"), None, 100);

        let history = vec![t2.clone(), t1.clone()]; // t2 seen first
        let res = heavy_rotation(&history, 2);
        assert_eq!(res[0].id.as_deref(), Some("id2"));
        assert_eq!(res[1].id.as_deref(), Some("id1"));
    }

    #[test]
    fn test_recent_discoveries() {
        let t1 = track(1, Some("id1"), Some(10), 100);
        let t2 = track(2, Some("id2"), Some(20), 100);
        let t3 = track(3, Some("id3"), None, 100); // oldest

        let lib = vec![t1, t2, t3];

        let res = recent_discoveries(&lib, 3);
        assert_eq!(res[0].id.as_deref(), Some("id2"));
        assert_eq!(res[1].id.as_deref(), Some("id1"));
        assert_eq!(res[2].id.as_deref(), Some("id3"));

        assert_eq!(recent_discoveries(&lib, 1).len(), 1);
        assert!(recent_discoveries(&[], 5).is_empty());
    }

    #[test]
    fn test_short_and_sweet() {
        let t1 = track(1, Some("id1"), None, 100);
        let t2 = track(2, Some("id2"), None, 200);
        let t3 = track(3, Some("id3"), None, 179);

        let lib = vec![t1, t2, t3];

        let res = short_and_sweet(&lib, 3);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].id.as_deref(), Some("id1"));
        assert_eq!(res[1].id.as_deref(), Some("id3"));

        assert_eq!(short_and_sweet(&lib, 1).len(), 1);
        assert!(short_and_sweet(&[], 5).is_empty());
    }

    #[test]
    fn test_forgotten_favorites() {
        let t1 = track(1, Some("id1"), Some(10), 100);
        let t2 = track(2, Some("id2"), Some(20), 100);
        let t3 = track(3, Some("id3"), None, 100); // oldest
        let t4 = track(4, None, Some(5), 100); // None id never in history

        let lib = vec![t1.clone(), t2.clone(), t3.clone(), t4.clone()];
        let history = vec![t1.clone()]; // id1 in history

        let res = forgotten_favorites(&lib, &history, 5);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].id.as_deref(), Some("id3")); // None added_at
        assert_eq!(res[1].id, None); // 5 added_at
        assert_eq!(res[2].id.as_deref(), Some("id2")); // 20 added_at

        assert_eq!(forgotten_favorites(&lib, &history, 1).len(), 1);
        assert!(forgotten_favorites(&[], &history, 5).is_empty());
    }
}
