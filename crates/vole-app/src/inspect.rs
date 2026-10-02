use vole_core::{Architecture, Instruction, Snapshot};

pub fn parse_hex(text: &str) -> Result<u64, String> {
    let text = text.trim().replace([' ', '_'], "");
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(&text);
    let text = text
        .strip_suffix('h')
        .or_else(|| text.strip_suffix('H'))
        .unwrap_or(text);
    if text.is_empty() {
        return Err("Enter a hexadecimal value, such as 7D or 0x7D.".into());
    }
    u64::from_str_radix(text, 16)
        .map_err(|_| "Use up to 16 hexadecimal digits (0–9 and A–F).".into())
}
pub fn integer(
    snapshot: &Snapshot,
    address: u64,
    width: usize,
    little_endian: bool,
) -> Option<u64> {
    let bytes = snapshot.read(address, width)?;
    Some(if little_endian {
        bytes
            .iter()
            .enumerate()
            .fold(0, |n, (i, b)| n | (u64::from(*b) << (i * 8)))
    } else {
        bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b))
    })
}
pub fn signed(value: u64, bits: usize) -> i64 {
    if bits == 64 {
        value as i64
    } else {
        ((value << (64 - bits)) as i64) >> (64 - bits)
    }
}
pub fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| {
            if (32..=126).contains(b) {
                char::from(*b)
            } else {
                '·'
            }
        })
        .collect()
}
pub fn vole_float(byte: u8) -> f64 {
    let sign = if byte & 0x80 == 0 { 1. } else { -1. };
    let exponent = i32::from((byte >> 4) & 7) - 4;
    let fraction = f64::from(byte & 15) / 16.;
    sign * fraction * 2f64.powi(exponent)
}
pub fn binary(value: u64, bits: usize) -> String {
    let text = format!("{value:0bits$b}");
    text.as_bytes()
        .chunks(8)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn vole_preview(instruction: &Instruction, snapshot: &Snapshot) -> Option<String> {
    if snapshot.architecture != Architecture::Vole
        || instruction.bytes.len() != 2
        || snapshot.halted
    {
        return None;
    }
    let word = u16::from_be_bytes([instruction.bytes[0], instruction.bytes[1]]);
    let opcode = word >> 12;
    if !matches!(opcode, 5 | 7 | 8 | 9) {
        return None;
    }
    let register = (word >> 8) & 15;
    let a = snapshot.register(&format!("R{:X}", (word >> 4) & 15))? as u8;
    let b = snapshot.register(&format!("R{:X}", word & 15))? as u8;
    let (operator, result) = match opcode {
        5 => ("+", a.wrapping_add(b)),
        7 => ("OR", a | b),
        8 => ("AND", a & b),
        9 => ("XOR", a ^ b),
        _ => return None,
    };
    Some(format!(
        "Preview: R{register:X} = {a:02X} {operator} {b:02X} = {result:02X}"
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_and_signed_preserve_full_width() {
        assert_eq!(parse_hex("0xFFFF_FFFF_FFFF_FFFF"), Ok(u64::MAX));
        assert_eq!(signed(0xff, 8), -1);
        assert!(parse_hex("10000000000000000").is_err());
    }
    #[test]
    fn vole_float_layout() {
        assert_eq!(vole_float(0x48), 0.5);
        assert_eq!(vole_float(0xc8), -0.5);
    }
}
