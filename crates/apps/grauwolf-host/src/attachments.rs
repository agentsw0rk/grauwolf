//! Files as attachments to the input of a run (ADR-0081).

use std::path::Path;

use base64::Engine;
use grauwolf_prompt::{Attachment, AttachmentData};

/// Maximum size of a file as an attachment.
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// Reads `file` as an attachment; the media type follows from the extension (png, jpg/jpeg, gif,
/// webp, pdf, wav, mp3). Unknown extensions, files that are too large or unreadable are errors.
pub fn from_file(file: &Path) -> Result<Attachment, String> {
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let media_type = media_type(&extension).ok_or_else(|| {
        format!(
            "{}: extension .{extension} is not supported as an attachment (possible: png, jpg, jpeg, gif, webp, pdf, wav, mp3)",
            file.display()
        )
    })?;
    let size = std::fs::metadata(file)
        .map_err(|error| format!("{}: {error}", file.display()))?
        .len();
    if size > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "{}: larger than 20 MB ({size} bytes)",
            file.display()
        ));
    }
    let bytes = std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(Attachment {
        media_type: media_type.into(),
        data: AttachmentData::Base64(base64::engine::general_purpose::STANDARD.encode(bytes)),
    })
}

/// Media type for a file extension (lowercase).
fn media_type(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        _ => return None,
    })
}
