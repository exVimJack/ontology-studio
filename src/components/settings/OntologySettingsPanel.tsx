// 本体设置：W3C 新建本体 IRI 前缀（业务创建时只填编码）。

import { useEffect, useState } from "react";
import { Boxes, X } from "lucide-react";

import {
  DEFAULT_ONTOLOGY_IRI_PREFIX,
  isValidOntologyIriPrefix,
  normalizeOntologyIriPrefix,
} from "@/lib/ontology-iri";
import { useUiStore } from "@/stores/ui-store";

export function OntologySettingsModal({ onClose }: { onClose: () => void }) {
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-4">
      <div className="flex max-h-[90vh] w-full max-w-lg flex-col rounded-lg border border-border bg-bg shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-2">
          <h2 className="flex items-center gap-1.5 text-sm font-semibold">
            <Boxes size={14} className="text-accent" />
            本体设置
          </h2>
          <button
            onClick={onClose}
            className="rounded-md p-1 text-fg-subtle hover:bg-bg-hover"
          >
            <X size={16} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          <OntologySettingsPanel onSaved={onClose} />
        </div>
      </div>
    </div>
  );
}

export function OntologySettingsPanel({ onSaved }: { onSaved?: () => void }) {
  const saved = useUiStore((s) => s.ontologyIriPrefix);
  const setPrefix = useUiStore((s) => s.setOntologyIriPrefix);
  const [draft, setDraft] = useState(saved);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(saved);
  }, [saved]);

  const valid = isValidOntologyIriPrefix(draft);

  function save() {
    if (!valid) {
      setError("前缀须为 http(s) 开头的合法 URL");
      return;
    }
    setError(null);
    setPrefix(draft);
    onSaved?.();
  }

  function resetDefault() {
    setDraft(DEFAULT_ONTOLOGY_IRI_PREFIX);
    setError(null);
  }

  return (
    <div className="space-y-3">
      <div>
        <label className="block text-[11px] font-medium text-fg-subtle">
          W3C 本体 IRI 前缀
        </label>
        <p className="mt-0.5 text-[11px] text-fg-muted">
          新建本体时业务只填编码；完整 IRI 为「前缀 + 编码 + #」。
        </p>
        <input
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value);
            setError(null);
          }}
          placeholder={DEFAULT_ONTOLOGY_IRI_PREFIX}
          className="mt-1.5 w-full rounded-md border border-border bg-bg-base px-2 py-1.5 text-xs outline-none"
        />
        <p className="mt-1 break-all text-[10px] text-fg-subtle">
          预览：{normalizeOntologyIriPrefix(draft || DEFAULT_ONTOLOGY_IRI_PREFIX)}
          UsedCar#
        </p>
      </div>
      {error && <p className="text-[11px] text-danger">{error}</p>}
      <div className="flex items-center justify-end gap-2">
        <button
          type="button"
          onClick={resetDefault}
          className="rounded-md px-2 py-1.5 text-xs text-fg-subtle hover:bg-bg-hover"
        >
          恢复默认
        </button>
        <button
          type="button"
          onClick={save}
          disabled={!valid}
          className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:bg-accent/90 disabled:opacity-50"
        >
          保存
        </button>
      </div>
    </div>
  );
}
