// Tests of client.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::SseDecoder;

#[test]
fn events_split_across_chunks() {
    let mut decoder = SseDecoder::default();
    assert_eq!(decoder.feed(b"event: stdout\nda"), vec![]);
    assert_eq!(
        decoder.feed(b"ta: hello\n\nevent: exit\ndata: 0\n\n"),
        vec![
            ("stdout".to_string(), "hello".to_string()),
            ("exit".to_string(), "0".to_string()),
        ]
    );
}

#[test]
fn multibyte_character_split_across_chunks() {
    let mut decoder = SseDecoder::default();
    let bytes = "event: stdout\ndata: 🤘\n\n".as_bytes();
    let middle_of_emoji = bytes.len() - 4;
    assert_eq!(decoder.feed(&bytes[..middle_of_emoji]), vec![]);
    assert_eq!(
        decoder.feed(&bytes[middle_of_emoji..]),
        vec![("stdout".to_string(), "🤘".to_string())]
    );
}
