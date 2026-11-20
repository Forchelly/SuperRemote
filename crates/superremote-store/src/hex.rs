//! 十六进制编解码。只为配置文件里的密文可读可手改，不值得为此多引一个依赖。

pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("长度不是偶数：{}", text.len()));
    }

    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = digit(bytes[i])?;
        let lo = digit(bytes[i + 1])?;
        out.push(hi << 4 | lo);
        i += 2;
    }
    Ok(out)
}

fn digit(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("非法字符：{:?}", byte as char)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_uppercase_input() {
        let bytes = [0x00, 0x0f, 0xf0, 0xff, 0x7a];
        let text = encode(&bytes);
        assert_eq!(text, "000ff0ff7a");
        assert_eq!(decode(&text).unwrap(), bytes);
        assert_eq!(decode("000FF0FF7A").unwrap(), bytes);
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(decode("0").is_err());
        assert!(decode("zz").is_err());
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }
}
