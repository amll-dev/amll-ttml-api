/// 为歌词文件生成唯一 ID，采用 FNV-1a 64-bit 哈希算法并截断到 53 位
#[expect(clippy::unreadable_literal)]
pub fn generate_file_id(filename: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;

    for byte in filename.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash & 0x1FFFFFFFFFFFFF
}
