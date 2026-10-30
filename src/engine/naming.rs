use super::*;

#[derive(Clone)]
pub(super) struct FilenameHint {
    pub name: String,
    pub priority: u8,
}

pub(super) fn generated_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_matches(['{', '}']);
    let stem = stem
        .rsplit_once(" (")
        .filter(|(_, suffix)| {
            suffix
                .strip_suffix(')')
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map_or(stem, |(base, _)| base);
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "" | "download" | "file" | "attachment"
    ) || ((32..=64).contains(&stem.len()) && stem.bytes().all(|c| c.is_ascii_hexdigit()))
        || (stem.len() == 36
            && stem.bytes().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            }))
}

fn safe_component(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next()?.trim();
    (!name.is_empty() && name != "." && name != "..").then(|| sanitize_filename(name))
}

fn disposition_name(header: &str) -> Option<String> {
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    let mut fields = Vec::new();
    for (i, c) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                fields.push(&header[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return None;
    }
    fields.push(&header[start..]);
    let mut ordinary = None;
    let mut extended = None;
    let (mut plain_seen, mut extended_seen) = (false, false);
    for field in fields.into_iter().skip(1) {
        let Some((key, raw)) = field.trim().split_once('=') else {
            continue;
        };
        let value = raw.trim();
        if key.trim().eq_ignore_ascii_case("filename*") {
            if extended_seen {
                return None;
            }
            extended_seen = true;
            let Some((charset, rest)) = value.split_once('\'') else {
                continue;
            };
            let Some((_, encoded)) = rest.split_once('\'') else {
                continue;
            };
            if encoded.as_bytes().iter().enumerate().any(|(i, b)| {
                *b == b'%'
                    && (i + 2 >= encoded.len()
                        || !encoded.as_bytes()[i + 1..i + 3]
                            .iter()
                            .all(u8::is_ascii_hexdigit))
            }) {
                continue;
            }
            let bytes = percent_encoding::percent_decode_str(encoded).collect::<Vec<_>>();
            let decoded = if charset.eq_ignore_ascii_case("utf-8") {
                String::from_utf8(bytes).ok()
            } else if charset.eq_ignore_ascii_case("iso-8859-1") {
                Some(bytes.into_iter().map(char::from).collect())
            } else {
                None
            };
            extended = decoded.and_then(|name| safe_component(&name));
        } else if key.trim().eq_ignore_ascii_case("filename") {
            if plain_seen {
                return None;
            }
            plain_seen = true;
            let value = if let Some(quoted) = value.strip_prefix('"') {
                quoted
                    .strip_suffix('"')?
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\")
            } else {
                value.to_owned()
            };
            ordinary = safe_component(&value);
        }
    }
    extended.or(ordinary)
}

pub(super) fn response_name(
    response: &reqwest::Response,
    download: &Download,
) -> Option<FilenameHint> {
    if download.preserve_name {
        return None;
    }
    if let Some(name) = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|h| std::str::from_utf8(h.as_bytes()).ok())
        .and_then(disposition_name)
    {
        return Some(FilenameHint { name, priority: 3 });
    }
    let redirected = filename(response.url());
    if generated_name(&download.name) && !generated_name(&redirected) {
        return Some(FilenameHint {
            name: redirected,
            priority: 2,
        });
    }
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()?
        .trim();
    if mime.eq_ignore_ascii_case("application/octet-stream") {
        return None;
    }
    let extension = mime_guess::get_mime_extensions_str(mime)?.first()?;
    let path = Path::new(&download.name);
    if generated_name(&download.name)
        && (path.extension().is_none()
            || path.extension().is_some_and(|ext| {
                matches!(
                    ext.to_str(),
                    Some("php" | "asp" | "aspx" | "jsp" | "cgi" | "bin" | "tmp" | "part")
                )
            }))
    {
        let stem = path.file_stem()?.to_string_lossy();
        return Some(FilenameHint {
            name: sanitize_filename(&format!("{stem}.{extension}")),
            priority: 1,
        });
    }
    None
}

impl Manager {
    pub(super) fn adopt_response_name(&self, job: &Arc<Job>) -> Result<()> {
        let Some(hint) = job.name_hint.lock().unwrap().clone() else {
            return Ok(());
        };
        let jobs = self.jobs.lock().unwrap();
        let current = job.data.lock().unwrap().clone();
        if current.preserve_name || current.name == hint.name {
            return Ok(());
        }
        let reserved = jobs
            .iter()
            .filter_map(|other| {
                let data = other.data.lock().unwrap();
                (data.id != current.id).then(|| data.path())
            })
            .collect::<Vec<_>>();
        let name = available_name(&current.directory, &hint.name, &reserved)?;
        job.data.lock().unwrap().name = name;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{disposition_name, generated_name};

    #[test]
    fn reads_safe_server_filenames_and_prefers_encoded_unicode() {
        for (header, expected) in [
            ("attachment; filename=report.pdf", Some("report.pdf")),
            (
                "attachment; filename=\"report; final.pdf\"",
                Some("report; final.pdf"),
            ),
            (
                "attachment; filename=ascii.pdf; filename*=UTF-8''caf%C3%A9.pdf",
                Some("café.pdf"),
            ),
            (
                "attachment; filename*=iso-8859-1''caf%E9.pdf",
                Some("café.pdf"),
            ),
            (
                "attachment; filename=\"../../CON.txt\"",
                Some("download_CON.txt"),
            ),
            ("attachment; filename=a.txt; filename=b.txt", None),
            (
                "attachment; filename=ok.txt; filename*=UTF-8''bad%ZZ.pdf",
                Some("ok.txt"),
            ),
            ("attachment; filename=\"unterminated", None),
        ] {
            assert_eq!(disposition_name(header).as_deref(), expected, "{header}");
        }
        assert!(generated_name("0976f308-1892-4a34-aeca-7b155f43121e.zip"));
        assert!(!generated_name("report.pdf"));
    }
}
