//! Binary protocol shared with the ESP32 firmware.
//!
//! Frame: `A5 5A | type u8 | len u16 LE | payload | xor(type, len, payload)`
//! All numbers are little-endian.

pub const T_TIME: u8 = 0x01;
pub const T_SYS: u8 = 0x02;
pub const T_CAVA: u8 = 0x03;
pub const T_WEATHER: u8 = 0x04;
pub const T_CONFIG: u8 = 0x05;
pub const T_PAGE: u8 = 0x06;
pub const T_THEME: u8 = 0x07;
pub const T_LOGO: u8 = 0x08;
pub const T_MEDIA: u8 = 0x09;
pub const T_LYRICS: u8 = 0x0A;
pub const T_LAYOUT: u8 = 0x0B;

/// Wrap a payload into a complete frame ready to be written to the serial port.
pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u16;
    let [lo, hi] = len.to_le_bytes();
    let mut out = Vec::with_capacity(payload.len() + 6);
    out.extend_from_slice(&[0xA5, 0x5A, kind, lo, hi]);
    out.extend_from_slice(payload);
    let chk = payload.iter().fold(kind ^ lo ^ hi, |acc, b| acc ^ b);
    out.push(chk);
    out
}

/// Small helper to build payloads field by field.
#[derive(Default)]
pub struct Buf(pub Vec<u8>);

impl Buf {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    /// Length-prefixed ASCII string, cut to `max` bytes.
    /// Non-ASCII characters become '?' because the ESP32 fonts are ASCII only.
    pub fn str(&mut self, s: &str, max: usize) -> &mut Self {
        let bytes: Vec<u8> = s.chars().filter_map(ascii_fold).take(max).collect();
        self.0.push(bytes.len() as u8);
        self.0.extend_from_slice(&bytes);
        self
    }
}

/// Map a char to something the ESP32's ASCII fonts can draw.
/// Common accented Latin letters lose their accent; other scripts become '?'.
pub fn ascii_fold(c: char) -> Option<u8> {
    if c.is_ascii() {
        return if c.is_ascii_control() { None } else { Some(c as u8) };
    }
    let r = match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'À'..='Å' | 'Ā' => 'A',
        'ç' | 'ć' | 'č' => 'c',
        'Ç' | 'Ć' | 'Č' => 'C',
        'è'..='ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'È'..='Ë' | 'Ē' | 'Ě' => 'E',
        'ì'..='ï' | 'ī' | 'ı' => 'i',
        'Ì'..='Ï' | 'Ī' | 'İ' => 'I',
        'ñ' | 'ń' | 'ň' => 'n',
        'Ñ' | 'Ń' => 'N',
        'ò'..='ö' | 'ø' | 'ō' | 'ő' => 'o',
        'Ò'..='Ö' | 'Ø' | 'Ō' => 'O',
        'ù'..='ü' | 'ū' | 'ů' | 'ű' => 'u',
        'Ù'..='Ü' | 'Ū' => 'U',
        'ý' | 'ÿ' => 'y',
        'Ý' | 'Ÿ' => 'Y',
        'š' | 'ś' | 'ş' => 's',
        'Š' | 'Ś' | 'Ş' => 'S',
        'ž' | 'ź' | 'ż' => 'z',
        'Ž' | 'Ź' | 'Ż' => 'Z',
        'ß' => 's',
        '‘' | '’' | '´' | '`' => '\'',
        '“' | '”' => '"',
        '–' | '—' => '-',
        '…' => '.',
        '\u{a0}' => ' ',
        _ => '?',
    };
    Some(r as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout_and_checksum() {
        let f = frame(T_CAVA, &[2, 10, 20]);
        assert_eq!(&f[..5], &[0xA5, 0x5A, 0x03, 3, 0]);
        assert_eq!(&f[5..8], &[2, 10, 20]);
        assert_eq!(f[8], 0x03 ^ 3 ^ 0 ^ 2 ^ 10 ^ 20);
    }

    #[test]
    fn string_is_ascii_and_truncated() {
        let mut b = Buf::default();
        b.str("Café°", 20);
        assert_eq!(b.0, vec![5, b'C', b'a', b'f', b'e', b'?']);
        let mut b = Buf::default();
        b.str("abcdefgh", 3);
        assert_eq!(b.0, vec![3, b'a', b'b', b'c']);
    }
}
