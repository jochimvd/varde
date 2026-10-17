use gio::prelude::*;

pub async fn read_all(input: gio::InputStream) -> Result<String, String> {
    let bytes = read_bytes(input).await?;
    String::from_utf8(bytes).map_err(|_| "input must be UTF-8".to_string())
}

pub async fn read_lines(input: gio::InputStream) -> Result<Vec<String>, String> {
    parse_lines(read_bytes(input).await?)
}

async fn read_bytes(input: gio::InputStream) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    loop {
        let chunk = input
            .read_bytes_future(64 * 1024, gio::glib::Priority::DEFAULT)
            .await
            .map_err(|error| format!("could not read input: {error}"))?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_lines(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    let input = String::from_utf8(bytes).map_err(|_| "selector input must be UTF-8".to_string())?;
    Ok(input.lines().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_duplicate_and_empty_selector_lines() {
        assert_eq!(
            parse_lines(b"One\n\nOne\n".to_vec()).unwrap(),
            ["One", "", "One"]
        );
        assert!(parse_lines(Vec::new()).unwrap().is_empty());
        assert!(parse_lines(vec![0xff]).is_err());
    }
}
