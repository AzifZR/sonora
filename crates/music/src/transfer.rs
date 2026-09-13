pub fn normalize(text: &str) -> String {
    let mut depth = 0usize;
    text.split(" - ")
        .next()
        .unwrap_or(text)
        .chars()
        .filter(|letter| match letter {
            '(' | '[' => {
                depth += 1;
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .filter(|letter| letter.is_alphanumeric() || letter.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn match_track(source: &crate::Track, candidates: &[crate::Track]) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }

    let source_title = normalize(&source.name);
    let source_artists: std::collections::HashSet<_> = source
        .artists
        .split([',', '&', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize)
        .collect();

    let mut best_match = None;
    let mut max_score = -1;

    for (index, candidate) in candidates.iter().enumerate() {
        let candidate_title = normalize(&candidate.name);

        if candidate_title != source_title {
            continue;
        }

        let candidate_artists: std::collections::HashSet<_> = candidate
            .artists
            .split([',', '&', ';'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(normalize)
            .collect();

        let mut score = 0;

        if !source_artists.is_disjoint(&candidate_artists) {
            score += 10;
        }

        let source_secs = source.duration.as_secs_f64();
        let candidate_secs = candidate.duration.as_secs_f64();

        if source_secs > 0.0 {
            let diff = (source_secs - candidate_secs).abs();
            if diff <= 60.0 {
                let tolerance = 5.0f64.max(source_secs * 0.03);
                if diff <= tolerance {
                    score += 5;
                }

                if score > max_score {
                    max_score = score;
                    best_match = Some(index);
                }
            }
        } else {
            if score > max_score {
                max_score = score;
                best_match = Some(index);
            }
        }
    }

    best_match
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Track;
    use std::time::Duration;

    fn make_track(name: &str, artists: &str, seconds: u64) -> Track {
        Track {
            id: Some("test_id".to_string()),
            name: name.to_string(),
            playable: true,
            artists: artists.to_string(),
            artist_refs: vec![],
            album: "album".to_string(),
            album_id: None,
            cover: None,
            duration: Duration::from_secs(seconds),
            added_at: None,
            added_by: None,
            playcount: None,
            popularity: 0,
            explicit: false,
            track_number: 1,
            disc_number: 1,
            tags: vec![],
            languages: vec![],
            credits: vec![],
        }
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize("HERE COMES THE SUN!!"), "here comes the sun");
        assert_eq!(normalize("here comes the sun"), "here comes the sun");
        assert_eq!(normalize("Song (feat. Artist)"), "song");
        assert_eq!(normalize("Song [Live]"), "song");
        assert_eq!(normalize("Song - Remastered 2024"), "song");
        assert_eq!(normalize("  spaced   words  "), "spaced words");
    }

    #[test]
    fn test_match_track_exact() {
        let source = make_track("Song", "Artist", 200);
        let candidates = vec![
            make_track("Other", "Other", 200),
            make_track("Song", "Artist", 200),
        ];
        assert_eq!(match_track(&source, &candidates), Some(1));
    }

    #[test]
    fn test_match_track_no_candidates() {
        let source = make_track("Song", "Artist", 200);
        let candidates = vec![];
        assert_eq!(match_track(&source, &candidates), None);
    }

    #[test]
    fn test_match_track_wrong_title_same_artist() {
        let source = make_track("Song", "Artist", 200);
        let candidates = vec![make_track("Other", "Artist", 200)];
        assert_eq!(match_track(&source, &candidates), None);
    }

    #[test]
    fn test_match_track_duration_far_off() {
        let source = make_track("Song", "Artist", 200);
        let candidates = vec![make_track("Song", "Artist", 210)]; // outside tolerance, within 60s
        assert_eq!(match_track(&source, &candidates), Some(0));
    }

    #[test]
    fn test_match_track_duration_too_far() {
        let source = make_track("Song", "Artist", 200);
        let candidates = vec![make_track("Song", "Artist", 270)]; // > 60s
        assert_eq!(match_track(&source, &candidates), None);
    }
}
