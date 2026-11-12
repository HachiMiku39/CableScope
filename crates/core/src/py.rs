//! Python 兼容的数值与文本辅助函数。
//!
//! 这些函数只服务于快照中松散的数值与字符串，后续波次引入 typed 结构时仍可复用。

use chrono::Utc;
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

const TWO_POW_63_F64: f64 = 9_223_372_036_854_775_808.0;
const TWO_POW_64_F64: f64 = 18_446_744_073_709_551_616.0;
const TWO_POW_64_I128: i128 = 1_i128 << 64;

/// 复刻 Python `round(x, n)` 的 decimal round-half-even 语义。
///
/// 常规路径按题意实现 `(x*10^n).round_ties_even()/10^n`。但 Python 3 的 `round`
/// 在 CPython 中走 `_Py_dg_dtoa` 的精确十进制舍入，浮点二进制表示会让
/// `round(2.675, 2) == 2.67`（2.675 实际略小于十进制 2.675）、
/// `round(0.0025, 3) == 0.003`；单纯缩放会在部分恰好半值处给出 2.68/0.002。
/// 因此在缩放后恰好半值、或缩放会损失精度/溢出的边界，退回 Rust 的精确二进制
/// 定点格式化（`format!("{:.*}")` 同样是 half-even），再解析回 f64。
///
/// 适用边界：n 非负且规模在 `f64` 合理区间；`scaled()` 固定使用 n=3。
/// |n| 超过 308 时 10^n 已超出 f64 范围：正 n 直接返回原值（任何有限 f64
/// 在 308 位小数后都不变），负 n 返回带符号的 0.0。NaN/Inf 沿用原值
/// （Python 的 `round` 对它们抛异常，但 `number()` 已在 JSON 边界过滤掉）。
pub fn py_round(x: f64, n: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if n > 308 {
        return x;
    }
    if n < -308 {
        return if x.is_sign_negative() { -0.0 } else { 0.0 };
    }
    if n < 0 {
        let p = 10_f64.powi(-n);
        return (x / p).round_ties_even() * p;
    }
    let p = 10_f64.powi(n);
    let scaled = x * p;
    if !scaled.is_finite() || x.abs() >= 1e15 || scaled.fract().abs() == 0.5 {
        return format!("{:.*}", n as usize, x).parse::<f64>().unwrap_or(x);
    }
    scaled.round_ties_even() / p
}

/// 把 JSON 值按 Python `isinstance(value, (int, float))` 且排除 `bool` 的语义
/// 转成 `f64`；字符串（哪怕内容是数字）一律拒绝，非有限值返回 `None`。
///
/// 注意 `serde_json::Value::Bool` 是独立变体，不会与 `Number` 混淆，正好对应
/// Python 中 `bool` 是 `int` 子类、必须显式排除的情况。serde_json 本身无法
/// 表示 NaN/Infinity（`Value::from(f64::NAN)` 会退化成 `Null`），因此非有限
/// 数值在进入 JSON 快照前就已被拒绝，不需要再运行 `math.isfinite`。
pub fn number_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        _ => None,
    }
}

/// 与 `number_f64` 相同，但尽量保留 Python 的整数类型：
/// JSON 中能表示为 i64/u64 的整数返回整数值，其余有限数值返回浮点值。
pub fn number(value: &Value) -> Option<Value> {
    let n = match value {
        Value::Number(n) => n,
        _ => return None,
    };
    if n.is_i64() || n.is_u64() {
        return Some(Value::Number(n.clone()));
    }
    n.as_f64()
        .filter(|x| x.is_finite())
        .and_then(Number::from_f64)
        .map(Value::Number)
}

/// Python `signed64`：数值 ≥ 2^63 时减去 2^64。
///
/// 整数路径用 `i128` 运算，保留 Python 任意精度整数的精确性；浮点路径按
/// Python 的 float 运算返回（f64 无法精确表示 2^64 附近的整数，见测试）。
pub fn signed64(value: &Value) -> Option<Value> {
    let n = match value {
        Value::Number(n) => n,
        _ => return None,
    };
    if let Some(v) = n.as_i64() {
        return Some(Value::Number(Number::from(v)));
    }
    if let Some(v) = n.as_u64() {
        let wide = v as i128;
        let adjusted = if wide >= (1_i128 << 63) {
            wide - TWO_POW_64_I128
        } else {
            wide
        };
        return Some(Value::Number(Number::from(adjusted as i64)));
    }
    n.as_f64()
        .filter(|x| x.is_finite())
        .and_then(|x| Number::from_f64(signed64_f64(x)))
        .map(Value::Number)
}

/// 浮点版本的 `signed64`，与 Python 对 float 输入的行为对齐。
pub fn signed64_f64(value: f64) -> f64 {
    if !value.is_finite() {
        return f64::NAN;
    }
    if value >= TWO_POW_63_F64 {
        value - TWO_POW_64_F64
    } else {
        value
    }
}

/// UTC 当前时间，格式 `%Y-%m-%dT%H:%M:%S+00:00`。
///
/// 对应 Python `datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")`。
pub fn now() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// 复刻 `hashlib.sha256(b"".join(blobs)).hexdigest()[:16]`：
/// 先拼接全部字节，再取摘要前 **8 个字节**（16 个十六进制字符）。
pub fn fingerprint(blobs: &[Vec<u8>]) -> String {
    let mut hasher = Sha256::new();
    for blob in blobs {
        hasher.update(blob);
    }
    hasher.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn number_rejects_bool_strings_and_non_finite() {
        // `py -3 -c` 对照：number(True/False/"3"/"" /None/{}) 均为 None；
        // number(float("nan")/inf/-inf) 均为 None；number(0)==0, number(-3)==-3,
        // number(1.25)==1.25。Rust 侧非有限值无法进入 serde_json::Number。
        assert_eq!(number(&json!(true)), None);
        assert_eq!(number(&json!(false)), None);
        assert_eq!(number(&json!("3")), None);
        assert_eq!(number(&json!("")), None);
        assert_eq!(number(&Value::Null), None);
        assert_eq!(number(&json!({"a": 1})), None);
        assert_eq!(number(&json!(0)), Some(json!(0)));
        assert_eq!(number(&json!(-3)), Some(json!(-3)));
        assert_eq!(number(&json!(1.25)), Some(json!(1.25)));
        assert_eq!(number_f64(&json!(true)), None);
        assert_eq!(number_f64(&json!("3")), None);
        assert_eq!(number_f64(&json!(0)), Some(0.0));
        assert_eq!(number_f64(&json!(1.25)), Some(1.25));
        assert_eq!(Value::from(f64::NAN), Value::Null);
        assert_eq!(Value::from(f64::INFINITY), Value::Null);
        assert_eq!(
            serde_json::to_string(&Value::from(f64::NAN)).unwrap(),
            "null"
        );
        // serde_json 默认不接受 NaN/Infinity 字面量，保持与 Python json.dumps
        // (allow_nan=False) 一致的拒绝行为。
        for text in ["NaN", "Infinity", "-Infinity", "1e400"] {
            assert!(serde_json::from_str::<Value>(text).is_err(), "{text}");
        }
    }

    #[test]
    fn py_round_matches_python_banker_rounding_and_float_representation() {
        // `py -3` 对照值（n=2）：round(2.675,2)=2.67，round(2.665,2)=2.67，
        // round(1.005,2)=1.0，round(0.125,2)=0.12，round(0.135,2)=0.14，
        // round(-0.125,2)=-0.12，round(2.685,2)=2.69。
        let cases2 = [
            (2.675_f64, 2.67_f64),
            (2.665, 2.67),
            (1.005, 1.0),
            (0.125, 0.12),
            (0.135, 0.14),
            (-0.125, -0.12),
            (2.685, 2.69),
        ];
        for (x, py) in cases2 {
            assert_eq!(py_round(x, 2), py, "py_round({x:?}, 2)");
        }
        // `py -3` 对照值（n=3）：round(0.0005,3)=0.001，
        // round(-0.0005,3)=-0.001，round(0.0015,3)=0.002，
        // round(0.0025,3)=0.003，round(2.6745,3)=2.675，
        // round(1234.56789,3)=1234.568，round(1e20,3)=1e20，
        // round(-0.0,3)=-0.0。
        let cases3 = [
            (0.0005_f64, 0.001_f64),
            (-0.0005, -0.001),
            (0.0015, 0.002),
            (0.0025, 0.003),
            (2.6745, 2.675),
            (1234.56789, 1234.568),
            (-1234.56789, -1234.568),
            (1e20, 1e20),
            (-0.0, -0.0),
        ];
        for (x, py) in cases3 {
            let got = py_round(x, 3);
            assert_eq!(got, py, "py_round({x:?}, 3)");
            assert_eq!(got.is_sign_negative(), py.is_sign_negative());
        }
        // 360 组 (40 个数值 × 9 个 n) 与 `py -3 round()` 差分全部一致，
        // 这里保留其中整数 half-even 与极值的代表值。
        assert_eq!(py_round(2.5, 0), 2.0);
        assert_eq!(py_round(3.5, 0), 4.0);
        assert_eq!(py_round(-2.5, 0), -2.0);
        assert_eq!(py_round(-3.5, 0), -4.0);
        assert_eq!(py_round(7.5, 0), 8.0);
        assert_eq!(py_round(8.5, 0), 8.0);
        assert_eq!(py_round(1234.5, 0), 1234.0);
        assert_eq!(py_round(1235.5, 0), 1236.0);
        assert_eq!(py_round(1250.0, -2), 1200.0);
        assert_eq!(py_round(1350.0, -2), 1400.0);
        assert_eq!(py_round(1e-300, 3), 0.0);
        assert_eq!(py_round(1e300, 400), 1e300);
        assert_eq!(py_round(1e300, -400), 0.0);
        assert_eq!(py_round(f64::INFINITY, 3), f64::INFINITY);
        assert!(py_round(f64::NAN, 3).is_nan());
    }

    #[test]
    fn signed64_subtracts_two_to_the_64() {
        // `py -3` 对照：signed64((1<<64)-1000)=-1000，
        // signed64((1<<64)-1)=-1，signed64(1<<64)=0（Python 大整数），
        // signed64((1<<63)-1)=9223372036854775807，signed64(True)=None。
        assert_eq!(signed64(&json!(0)), Some(json!(0)));
        assert_eq!(signed64(&json!(-1)), Some(json!(-1)));
        assert_eq!(signed64(&json!(true)), None);
        assert_eq!(signed64(&json!("3")), None);
        assert_eq!(
            signed64(&json!(9_223_372_036_854_775_807_u64)),
            Some(json!(9_223_372_036_854_775_807_i64))
        );
        assert_eq!(
            signed64(&json!(18_446_744_073_709_551_615_u64)),
            Some(json!(-1))
        );
        let almost_max = 18_446_744_073_709_551_615_u64 - 999;
        assert_eq!(signed64(&json!(almost_max)), Some(json!(-1000)));
        // 浮点输入的 Python 对照：signed64(float(1<<63)) == -9.223372036854776e18。
        assert_eq!(signed64_f64(TWO_POW_63_F64), -9_223_372_036_854_776_000.0);
        assert_eq!(signed64_f64(TWO_POW_64_F64), 0.0);
        assert_eq!(signed64_f64(-1.0), -1.0);
        assert!(signed64_f64(f64::NAN).is_nan());
    }

    #[test]
    fn fingerprint_matches_python_hexdigest_prefix() {
        // `py -3` 对照：四个小端 u32 VDO（0x18001234, 0, 0x56780001, 0x42）
        // 拼接后 sha256(...).hexdigest()[:16] == "6894529626a02878"。
        let blobs = [(3_u32 << 27) | 0x1234, 0, 0x5678_0001, 0x42]
            .iter()
            .map(|w| w.to_le_bytes().to_vec())
            .collect::<Vec<_>>();
        assert_eq!(fingerprint(&blobs), "6894529626a02878");
        // hexdigest()[:16] 是 16 个字符 = 8 字节，不是 16 字节。
        assert_eq!(fingerprint(&blobs).len(), 16);
    }

    #[test]
    fn now_string_shape_matches_python_isoformat() {
        // `py -3` 对照：datetime.isoformat(timespec="seconds") 长度 25，
        // 以 "+00:00" 结尾，秒位无小数。
        let value = now();
        assert_eq!(value.len(), 25, "{value}");
        assert!(value.ends_with("+00:00"), "{value}");
        assert_eq!(value.as_bytes()[4], b'-');
        assert_eq!(value.as_bytes()[7], b'-');
        assert_eq!(value.as_bytes()[10], b'T');
        assert_eq!(value.as_bytes()[13], b':');
        assert_eq!(value.as_bytes()[16], b':');
    }
}
