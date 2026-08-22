//! DataType 枚举（对齐 Gaia `core/schemas/ontology.py` DataType）。
//!
//! 属性 data_type 只能取这些值。Rust 侧校验 + DB CHECK 双重兜底。

/// DataType 枚举值（对齐 Gaia）。
pub const DATA_TYPES: &[&str] = &[
    "STRING",
    "INTEGER",
    "SHORT",
    "LONG",
    "BOOLEAN",
    "BYTE",
    "FLOAT",
    "DOUBLE",
    "DECIMAL",
    "DATE",
    "TIMESTAMP",
    "ARRAY",
    "STRUCT",
    "VECTOR",
    "GEOPOINT",
    "GEOSHAPE",
    "GEOTEMPORAL_SERIES",
    "TIME_SERIES",
    "MEDIA_REFERENCE",
    "ATTACHMENT",
];

/// 判定 data_type 是否为合法枚举值。
pub fn is_valid_data_type(s: &str) -> bool {
    DATA_TYPES.contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_types() {
        assert!(is_valid_data_type("STRING"));
        assert!(is_valid_data_type("TIMESTAMP"));
        assert!(is_valid_data_type("DECIMAL"));
        assert!(is_valid_data_type("LONG"));
    }

    #[test]
    fn unknown_types() {
        assert!(!is_valid_data_type("BIGINT"));
        assert!(!is_valid_data_type("INT"));
        assert!(!is_valid_data_type("INT64"));
        assert!(!is_valid_data_type(""));
        assert!(!is_valid_data_type("string")); // 大小写敏感
    }
}
