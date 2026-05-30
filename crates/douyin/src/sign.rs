//! a-bogus 签名：从 jiji262 `douyin-downloader/utils/abogus.py`（f2 库 ABogus 1.0.1.19）
//! 逐位移植的纯算法实现——SM3 + RC4 + 自定义 base64，**无 Node/JS 引擎依赖**。
//!
//! 正确性由 `tests` 中的黄金向量保证：固定 time / random 后，本实现对同一输入产出的
//! a_bogus 必须与 Python 版逐字节一致（向量见 `D:\git\_study\validate\golden.py` 生成）。
//!
//! 用法（与 jiji262 接法一致）：`query = urlencode(params)` → `Abogus::new(fp, ua).sign(&query, "")`
//! 返回 `(query, a_bogus)`，调用方拼成 `?{query}&a_bogus={a_bogus}` 即为最终 URL。

use sm3::{Digest, Sm3};

const SALT: &str = "cus";
const UA_KEY: [u8; 3] = [0x00, 0x01, 0x0e];
const CHARSET_0: &[u8] = b"Dkdpgh2ZmsQB80/MfvV36XI1R45-WUAlEixNLwoqYTOPuzKFjJnry79HbGcaStCe";
const CHARSET_1: &[u8] = b"ckdp1h4ZKsUB80/Mfvw36XIgR25+WQAlEi7NLboqYTOPuzmFjJnryx9HVGDaStCe";

/// GET / POST 共用 [0, 1, 14]（14 兼容 8，POST 同样能编码 params，与 f2 实现一致）。
const OPTIONS: [i64; 3] = [0, 1, 14];

#[rustfmt::skip]
const BIG_ARRAY: [u8; 256] = [
    121, 243,  55, 234, 103,  36,  47, 228,  30, 231, 106,   6, 115,  95,  78, 101, 250, 207, 198,  50,
    139, 227, 220, 105,  97, 143,  34,  28, 194, 215,  18, 100, 159, 160,  43,   8, 169, 217, 180, 120,
    247,  45,  90,  11,  27, 197,  46,   3,  84,  72,   5,  68,  62,  56, 221,  75, 144,  79,  73, 161,
    178,  81,  64, 187, 134, 117, 186, 118,  16, 241, 130,  71,  89, 147, 122, 129,  65,  40,  88, 150,
    110, 219, 199, 255, 181, 254,  48,   4, 195, 248, 208,  32, 116, 167,  69, 201,  17, 124, 125, 104,
     96,  83,  80, 127, 236, 108, 154, 126, 204,  15,  20, 135, 112, 158,  13,   1, 188, 164, 210, 237,
    222,  98, 212,  77, 253,  42, 170, 202,  26,  22,  29, 182, 251,  10, 173, 152,  58, 138,  54, 141,
    185,  33, 157,  31, 252, 132, 233, 235, 102, 196, 191, 223, 240, 148,  39, 123,  92,  82, 128, 109,
     57,  24,  38, 113, 209, 245,   2, 119, 153, 229, 189, 214, 230, 174, 232,  63,  52, 205,  86, 140,
     66, 175, 111, 171, 246, 133, 238, 193,  99,  60,  74,  91, 225,  51,  76,  37, 145, 211, 166, 151,
    213, 206,   0, 200, 244, 176, 218,  44, 184, 172,  49, 216,  93, 168,  53,  21, 183,  41,  67,  85,
    224, 155, 226, 242,  87, 177, 146,  70, 190,  12, 162,  19, 137, 114,  25, 165, 163, 192,  23,  59,
      9,  94, 179, 107,  35,   7, 142, 131, 239, 203, 149, 136,  61, 249,  14, 156,
];

#[rustfmt::skip]
const SORT_INDEX: [usize; 44] = [
    18, 20, 52, 26, 30, 34, 58, 38, 40, 53, 42, 21, 27, 54, 55, 31, 35, 57, 39, 41, 43, 22, 28,
    32, 60, 36, 23, 29, 33, 37, 44, 45, 59, 46, 47, 48, 49, 50, 24, 25, 65, 66, 70, 71,
];

#[rustfmt::skip]
const SORT_INDEX_2: [usize; 44] = [
    18, 20, 26, 30, 34, 38, 40, 42, 21, 27, 31, 35, 39, 41, 43, 22, 28, 32, 36, 23, 29, 33, 37,
    44, 45, 46, 47, 48, 49, 50, 24, 25, 52, 53, 54, 55, 57, 58, 59, 60, 65, 66, 70, 71,
];

/// SM3 哈希，返回 32 字节。对应 Python `sm3_to_array`。
fn sm3_bytes(input: &[u8]) -> [u8; 32] {
    let mut h = Sm3::new();
    h.update(input);
    h.finalize().into()
}

/// 字符串参数加盐后 SM3。对应 `params_to_array(str, add_salt=True)`。
fn hash_string(param: &str, add_salt: bool) -> [u8; 32] {
    if add_salt {
        let mut salted = String::with_capacity(param.len() + SALT.len());
        salted.push_str(param);
        salted.push_str(SALT);
        sm3_bytes(salted.as_bytes())
    } else {
        sm3_bytes(param.as_bytes())
    }
}

/// 双重 SM3：`params_to_array(params_to_array(s))`。内层加盐、外层对字节再哈希（不加盐）。
fn double_hash(s: &str) -> [u8; 32] {
    let inner = hash_string(s, true);
    sm3_bytes(&inner)
}

/// RC4。对应 `rc4_encrypt`。
fn rc4(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = [0; 256];
    for (i, v) in s.iter_mut().enumerate() {
        *v = i as u8;
    }
    let mut j: usize = 0;
    for i in 0..256 {
        j = (j + s[i] as usize + key[i % key.len()] as usize) % 256;
        s.swap(i, j);
    }
    let mut i: usize = 0;
    j = 0;
    let mut out = Vec::with_capacity(plaintext.len());
    for &c in plaintext {
        i = (i + 1) % 256;
        j = (j + s[i] as usize) % 256;
        s.swap(i, j);
        let k = s[(s[i] as usize + s[j] as usize) % 256];
        out.push(c ^ k);
    }
    out
}

/// 自定义 8-bit 分组 base64。对应 `base64_encode`，输入按字节处理。
fn base64_encode(input: &[u8], alphabet: usize) -> String {
    let table = if alphabet == 0 { CHARSET_0 } else { CHARSET_1 };
    let mut bits = String::with_capacity(input.len() * 8);
    for &b in input {
        bits.push_str(&format!("{:08b}", b));
    }
    let pad = (6 - bits.len() % 6) % 6;
    for _ in 0..pad {
        bits.push('0');
    }
    let mut out = String::new();
    let bytes = bits.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let chunk = &bits[i..i + 6];
        let idx = usize::from_str_radix(chunk, 2).unwrap_or(0);
        out.push(table[idx] as char);
        i += 6;
    }
    out.push_str(&"=".repeat(pad / 2));
    out
}

/// a-bogus 最终编码：3 字节 → 4 字符，带 f2 的 j==6/j==0 提前 break 逻辑。对应 `abogus_encode`。
fn abogus_encode(data: &[u8], alphabet: usize) -> String {
    let table = if alphabet == 0 { CHARSET_0 } else { CHARSET_1 };
    let mut out: Vec<u8> = Vec::new();
    let len = data.len();
    let mut i = 0;
    while i < len {
        let n: u32 = if i + 2 < len {
            ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32)
        } else if i + 1 < len {
            ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8)
        } else {
            (data[i] as u32) << 16
        };
        for (j, k) in [(18u32, 0xFC0000u32), (12, 0x03F000), (6, 0x0FC0), (0, 0x3F)] {
            if j == 6 && i + 1 >= len {
                break;
            }
            if j == 0 && i + 2 >= len {
                break;
            }
            out.push(table[((n & k) >> j) as usize]);
        }
        i += 3;
    }
    let pad = (4 - out.len() % 4) % 4;
    out.resize(out.len() + pad, b'=');
    String::from_utf8(out).unwrap_or_default()
}

/// JS 无符号右移。对应 `js_shift_right`。
fn js_shr(val: u64, n: u32) -> u64 {
    (val % 0x1_0000_0000) >> n
}

/// 12 字节伪随机前缀。对应 `generate_random_bytes`（length=3，每组 4 字节）。
/// `rds` 为 3 组的 `_rd` 值（= `int(random.random()*10000)`）。
fn random_prefix(rds: [u32; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    for rd in rds {
        let rd = rd as u64;
        out.push((((rd & 255) & 170) | 1) as u8);
        out.push((((rd & 255) & 85) | 2) as u8);
        out.push(((js_shr(rd, 8) & 170) | 5) as u8);
        out.push(((js_shr(rd, 8) & 85) | 40) as u8);
    }
    out
}

/// 对 `big_array` 的副本做置换加密。对应 `transform_bytes`。
fn transform_bytes(values: &[u8]) -> Vec<u8> {
    let mut ba: [i64; 256] = [0; 256];
    for (i, v) in BIG_ARRAY.iter().enumerate() {
        ba[i] = *v as i64;
    }
    let len = 256i64;
    let mut result = Vec::with_capacity(values.len());
    let mut index_b = ba[1];
    let mut initial_value: i64 = 0;
    let mut value_e: i64 = 0;

    for (index, &ch) in values.iter().enumerate() {
        let mut sum_initial: i64;
        if index == 0 {
            initial_value = ba[index_b as usize];
            ba[1] = initial_value;
            ba[index_b as usize] = index_b;
            sum_initial = index_b + initial_value;
        } else {
            sum_initial = initial_value + value_e;
        }
        sum_initial = sum_initial.rem_euclid(len);
        let value_f = ba[sum_initial as usize];
        let encrypted = (ch as i64) ^ value_f;
        result.push(encrypted as u8);

        let nxt = ((index as i64) + 2).rem_euclid(len) as usize;
        value_e = ba[nxt];
        let swap_idx = (index_b + value_e).rem_euclid(len) as usize;
        initial_value = ba[swap_idx];
        ba[swap_idx] = ba[nxt];
        ba[nxt] = initial_value;
        index_b = swap_idx as i64;
    }
    result
}

/// a-bogus 签名器。
pub struct Abogus {
    fp: String,
    user_agent: String,
}

impl Abogus {
    pub fn new(fp: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            fp: fp.into(),
            user_agent: user_agent.into(),
        }
    }

    /// 确定性核心：注入时间与随机，供黄金向量测试逐字节对照。
    fn generate_with(
        &self,
        params: &str,
        body: &str,
        start_ms: u64,
        end_ms: u64,
        rds: [u32; 3],
    ) -> String {
        let mut ab = [0i64; 72];
        ab[8] = 3;
        ab[18] = 44;

        let array1 = double_hash(params);
        let array2 = double_hash(body);
        let rc4_ua = rc4(&UA_KEY, self.user_agent.as_bytes());
        let array3 = hash_string(&base64_encode(&rc4_ua, 1), false);

        // 加密开始时间
        ab[20] = ((start_ms >> 24) & 255) as i64;
        ab[21] = ((start_ms >> 16) & 255) as i64;
        ab[22] = ((start_ms >> 8) & 255) as i64;
        ab[23] = (start_ms & 255) as i64;
        ab[24] = (start_ms / 256 / 256 / 256 / 256) as i64;
        ab[25] = (start_ms / 256 / 256 / 256 / 256 / 256) as i64;

        // 请求头配置 / 方法 / 头加密（options）
        ab[26] = (OPTIONS[0] >> 24) & 255;
        ab[27] = (OPTIONS[0] >> 16) & 255;
        ab[28] = (OPTIONS[0] >> 8) & 255;
        ab[29] = OPTIONS[0] & 255;
        ab[30] = (OPTIONS[1] / 256) & 255;
        ab[31] = (OPTIONS[1] % 256) & 255;
        ab[32] = (OPTIONS[1] >> 24) & 255;
        ab[33] = (OPTIONS[1] >> 16) & 255;
        ab[34] = (OPTIONS[2] >> 24) & 255;
        ab[35] = (OPTIONS[2] >> 16) & 255;
        ab[36] = (OPTIONS[2] >> 8) & 255;
        ab[37] = OPTIONS[2] & 255;

        // 请求体 / body / ua 哈希
        ab[38] = array1[21] as i64;
        ab[39] = array1[22] as i64;
        ab[40] = array2[21] as i64;
        ab[41] = array2[22] as i64;
        ab[42] = array3[23] as i64;
        ab[43] = array3[24] as i64;

        // 加密结束时间
        ab[44] = ((end_ms >> 24) & 255) as i64;
        ab[45] = ((end_ms >> 16) & 255) as i64;
        ab[46] = ((end_ms >> 8) & 255) as i64;
        ab[47] = (end_ms & 255) as i64;
        ab[48] = ab[8];
        ab[49] = (end_ms / 256 / 256 / 256 / 256) as i64;
        ab[50] = (end_ms / 256 / 256 / 256 / 256 / 256) as i64;

        // pageId = 0（固定），aid = 6383
        let aid: i64 = 6383;
        ab[51] = 0;
        ab[52] = 0;
        ab[53] = 0;
        ab[54] = 0;
        ab[55] = 0;
        ab[56] = aid;
        ab[57] = aid & 255;
        ab[58] = (aid >> 8) & 255;
        ab[59] = (aid >> 16) & 255;
        ab[60] = (aid >> 24) & 255;

        // 浏览器指纹长度
        let fp_len = self.fp.len() as i64;
        ab[64] = fp_len;
        ab[65] = fp_len;

        let mut sorted_values: Vec<u8> = SORT_INDEX.iter().map(|&i| ab[i] as u8).collect();

        // ab_xor：对 sort_index_2 链式异或
        let mut ab_xor: i64 = 0;
        for idx in 0..SORT_INDEX_2.len() - 1 {
            if idx == 0 {
                ab_xor = ab[SORT_INDEX_2[idx]];
            }
            ab_xor ^= ab[SORT_INDEX_2[idx + 1]];
        }

        sorted_values.extend(self.fp.bytes());
        sorted_values.push(ab_xor as u8);

        let mut payload = random_prefix(rds);
        payload.extend(transform_bytes(&sorted_values));

        abogus_encode(&payload, 0)
    }

    /// 生产用：真实时间 + 随机。返回 a_bogus 串。
    pub fn sign(&self, params: &str, body: &str) -> String {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // 随机 _rd：避免引入 rand 依赖，用纳秒抖动派生 3 组伪随机（仅用于混淆前缀）。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let rds = [nanos % 10000, (nanos / 7) % 10000, (nanos / 13) % 10000];
        self.generate_with(params, body, now_ms, now_ms, rds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0";
    const FP: &str = "1920|1080|1920|1040|0|0|0|0|1920|1040|1920|1080|24|24|Win32";
    const PARAMS: &str = "device_platform=webapp&aid=6383&channel=channel_pc_web&sec_user_id=MS4wLjABAAAAtest&max_cursor=0&count=18&version_code=290100";

    #[test]
    fn sm3_standard_vector_abc() {
        // SM3("abc") 标准向量
        let h = sm3_bytes(b"abc");
        assert_eq!(h[0], 0x66);
        assert_eq!(h[1], 0xc7);
        assert_eq!(h[2], 0xf0);
        assert_eq!(h[3], 0xf4);
        assert_eq!(
            &h[..],
            &[
                102, 199, 240, 244, 98, 238, 237, 217, 209, 242, 212, 107, 220, 16, 228, 226, 65,
                103, 196, 135, 92, 242, 247, 162, 41, 125, 160, 43, 143, 75, 168, 224,
            ]
        );
    }

    #[test]
    fn rc4_ua_golden() {
        let out = rc4(&UA_KEY, UA.as_bytes());
        let hex: String = out.iter().map(|b| format!("{:02x}", b)).collect();
        assert!(hex.starts_with("958694fdafe410a7311ed92c6d40c49b"));
        assert_eq!(hex.len(), UA.len() * 2);
    }

    #[test]
    fn base64_custom_golden() {
        assert_eq!(base64_encode(b"hello", 0), "52Xu-2S=");
        assert_eq!(base64_encode(b"hello", 1), "54Xu+4S=");
    }

    #[test]
    fn double_hash_golden() {
        let got = double_hash(PARAMS);
        assert_eq!(
            &got[..],
            &[
                19, 177, 251, 181, 28, 101, 31, 126, 64, 181, 30, 79, 93, 51, 49, 113, 212, 224,
                98, 64, 22, 175, 200, 174, 169, 229, 7, 11, 60, 60, 177, 133,
            ]
        );
    }

    #[test]
    fn e2e_get_abogus_golden() {
        let ab = Abogus::new(FP, UA);
        let got = ab.generate_with(
            PARAMS,
            "",
            1_700_000_000_123,
            1_700_000_000_123,
            [4242, 4242, 4242],
        );
        assert_eq!(
            got,
            "EJmh/m8Vk3xpgE6b56KLfY3q64P3YQxI0SVkMD2fFVfPqL39HMTa9exoIBGvXFEjwG/-IbDjy4hbO3xprQAjM36UHWwEUdQ2mgWkKl5Q5xSSs1feeLbQrsJx-kTlFeep5JV3EcvhqJKGKuRplnl60fAAPby="
        );
    }

    #[test]
    fn e2e_post_abogus_golden() {
        let ab = Abogus::new(FP, UA);
        let got = ab.generate_with(
            PARAMS,
            "aweme_id=123&action=1",
            1_700_000_000_123,
            1_700_000_000_123,
            [4242, 4242, 4242],
        );
        assert_eq!(
            got,
            "EJmh/m8Vk3xpgE6b56KLfY3q64Y/YQxI0SVkMD2fFlfPqL39HMTa9exoIBGvXFEjwG/-IbDjy4hbO3xprQAjM36UHWwEUdQ2mgWkKl5Q5xSSs1feeLbQrsJx-kTlFeep5JV3EcvhqJKGKuRplnl60fAAPbD="
        );
    }

    #[test]
    fn sign_appends_nonempty() {
        let ab = Abogus::new(FP, UA);
        let s = ab.sign(PARAMS, "");
        assert!(!s.is_empty());
        assert!(s.ends_with('='));
    }
}
