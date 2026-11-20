//! Files as attachments (ADR-0081).

use grauwolf_host::attachments::from_file;
use grauwolf_prompt::AttachmentData;

#[test]
fn reads_files_as_attachments_by_their_extension() {
    let dir = grauwolf_test::TestDir::new("attachments");
    std::fs::write(dir.join("wolf.PNG"), [0x89, b'P', b'N', b'G']).unwrap();
    std::fs::write(dir.join("note.txt"), "Hello").unwrap();
    std::fs::write(dir.join("huge.pdf"), vec![0_u8; 20 * 1024 * 1024 + 1]).unwrap();

    let picture = from_file(&dir.join("wolf.PNG")).unwrap();
    let text = from_file(&dir.join("note.txt")).unwrap_err();
    let huge = from_file(&dir.join("huge.pdf")).unwrap_err();
    let missing = from_file(&dir.join("missing.jpg")).unwrap_err();

    assert_eq!(picture.media_type, "image/png");
    assert_eq!(picture.data, AttachmentData::Base64("iVBORw==".into()));
    assert!(text.contains("note.txt") && text.contains(".txt"), "{text}");
    assert!(huge.contains("20 MB"), "{huge}");
    assert!(missing.contains("missing.jpg"), "{missing}");
}
