//! 命名规范（对齐 Gaia `core/naming.py`）。
//!
//! api_name pattern 真相源——所有实体的 api_name 校验都走这里。
//! Rust 侧预校验后再 INSERT，DB 约束（UNIQUE/FK）只做兜底。

use std::sync::OnceLock;

/// 属性 / LinkType / ActionType / 参数 api_name：camelCase，首词小写。
pub const PROPERTY_API_NAME_PATTERN: &str = r"^[a-z][a-zA-Z0-9]{0,99}$";
/// ObjectType / Ontology / ObjectTypeGroup api_name：PascalCase，首字母大写。
pub const OBJECT_TYPE_API_NAME_PATTERN: &str = r"^[A-Z][a-zA-Z0-9]{0,99}$";
/// Dataset / DataSource / Credential api_name：snake_case，全小写保词界（兼任物理表名）。
pub const DATASET_API_NAME_PATTERN: &str = r"^[a-z][a-z0-9_]{0,99}$";

struct Patterns {
    property: regex::Regex,
    object_type: regex::Regex,
    dataset: regex::Regex,
}

static PATTERNS: OnceLock<Patterns> = OnceLock::new();

fn patterns() -> &'static Patterns {
    PATTERNS.get_or_init(|| Patterns {
        property: regex::Regex::new(PROPERTY_API_NAME_PATTERN).unwrap(),
        object_type: regex::Regex::new(OBJECT_TYPE_API_NAME_PATTERN).unwrap(),
        dataset: regex::Regex::new(DATASET_API_NAME_PATTERN).unwrap(),
    })
}

/// 校验 ObjectType / Ontology / ObjectTypeGroup 的 api_name（PascalCase）。
pub fn is_valid_object_type_api_name(s: &str) -> bool {
    patterns().object_type.is_match(s)
}

/// 校验 Property / LinkType / ActionType / 参数 的 api_name（camelCase）。
pub fn is_valid_property_api_name(s: &str) -> bool {
    patterns().property.is_match(s)
}

/// 校验 Dataset / DataSource / Credential 的 api_name（snake_case）。
pub fn is_valid_dataset_api_name(s: &str) -> bool {
    patterns().dataset.is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case() {
        assert!(is_valid_object_type_api_name("PurchaseOrder"));
        assert!(is_valid_object_type_api_name("A"));
        assert!(!is_valid_object_type_api_name("purchaseOrder")); // 首字母小写
        assert!(!is_valid_object_type_api_name("Purchase_Order")); // 下划线
        assert!(!is_valid_object_type_api_name("")); // 空
    }

    #[test]
    fn camel_case() {
        assert!(is_valid_property_api_name("orderDate"));
        assert!(is_valid_property_api_name("a"));
        assert!(!is_valid_property_api_name("OrderDate")); // 首字母大写
        assert!(!is_valid_property_api_name("order_date")); // 下划线
    }

    #[test]
    fn snake_case() {
        assert!(is_valid_dataset_api_name("purchase_order"));
        assert!(is_valid_dataset_api_name("a"));
        assert!(!is_valid_dataset_api_name("PurchaseOrder")); // 大写
        assert!(!is_valid_dataset_api_name("1abc")); // 数字开头
    }
}
