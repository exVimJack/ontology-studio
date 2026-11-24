/** 设计宪章两字段的用户可见文案（W3C / Palantir / AI 起草共用）。 */

export type CharterFieldKey = "business_scenario" | "business_essence";

export const CHARTER_FIELD_META: {
  key: CharterFieldKey;
  label: string;
  hint: string;
  placeholder: string;
}[] = [
  {
    key: "business_scenario",
    label: "业务场景",
    hint: "定义本体建模的使用范围以及对象",
    placeholder: "例如：连接个人卖家、车商与买家的二手车交易平台……",
  },
  {
    key: "business_essence",
    label: "业务目标",
    hint: "定义本体建模的使用目标",
    placeholder: "例如：支撑非标实物可信交易撮合与履约跟踪……",
  },
];

export function charterFieldLabel(key: CharterFieldKey): string {
  return CHARTER_FIELD_META.find((f) => f.key === key)?.label ?? key;
}
