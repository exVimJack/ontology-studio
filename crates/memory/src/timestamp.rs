//! 时间戳类型：i64 内存表示，specta 导出为 TS number。
//!
//! 背景：specta-typescript 0.0.12 默认禁止 i64/u64 导出（BigInt 精度顾虑），
//! 但毫秒级 unix 时间戳 < 2^53，JS number 完全无损。故用 newtype 包装 i64，
//! 手动 impl specta::Type 输出 `number`，绕过限制。
//!
//! 一处定义，repo/message/provider 的 DTO 统一使用。

use serde::{Deserialize, Serialize};

/// 毫秒级 unix 时间戳。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        )
    }

    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for Timestamp {
    fn from(v: i64) -> Self {
        Self(v)
    }
}

impl From<Timestamp> for i64 {
    fn from(t: Timestamp) -> i64 {
        t.0
    }
}

// rusqlite：作为 INTEGER（i64）存取
impl rusqlite::ToSql for Timestamp {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl rusqlite::types::FromSql for Timestamp {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        i64::column_result(value).map(Self)
    }
}

// serde：作为数字（i64）序列化，前后端一致
impl Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(Self(i64::deserialize(de)?))
    }
}

// specta：导出为 TS `number`（非 bigint），绕过 i64 禁令。
// 借用 Primitive::i32 生成 "number" 标注——实际运行时传 i64 序列化的 JSON number，
// JS number（f64）存毫秒时间戳（< 2^53）完全无损。仅 TS 类型标注层面的权宜。
impl specta::Type for Timestamp {
    fn definition(_types: &mut specta::Types) -> specta::datatype::DataType {
        specta::datatype::DataType::Primitive(specta::datatype::Primitive::i32)
    }
}
