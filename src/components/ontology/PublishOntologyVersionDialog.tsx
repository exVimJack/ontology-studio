// 共享：发布本体版本弹窗（版本面板与概念图谱「去发布」共用）

import { useState } from "react";
import { X } from "lucide-react";
import { usePublishOntologyVersion } from "@/hooks/useOntologyTtl";
import styles from "./OntologyVersionsPanel.module.css";

export function PublishOntologyVersionDialog({
  ontologyIri,
  open,
  onClose,
  onPublished,
}: {
  ontologyIri: string;
  open: boolean;
  onClose: () => void;
  onPublished?: () => void;
}) {
  const publish = usePublishOntologyVersion();
  const [label, setLabel] = useState("");
  const [description, setDescription] = useState("");

  if (!open) return null;

  const doPublish = () => {
    publish.mutate(
      {
        iri: ontologyIri,
        label: label.trim(),
        description: description.trim(),
      },
      {
        onSuccess: () => {
          setLabel("");
          setDescription("");
          onPublished?.();
          onClose();
        },
      },
    );
  };

  return (
    <div className={styles.modalMask} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.modalHead}>
          <span>发布新版本</span>
          <button className={styles.iconBtn} onClick={onClose} title="关闭">
            <X size={14} />
          </button>
        </div>
        <div className={styles.modalBody}>
          <label className={styles.fieldLabel}>版本标签</label>
          <input
            className={styles.input}
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="留空自动生成 v0.0.N（N=发布序号）；填了以你的为准"
          />
          <label className={styles.fieldLabel}>发布说明</label>
          <textarea
            className={styles.textarea}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="如：采购本体-增加审批流"
            rows={3}
          />
          <div className={styles.modalFoot}>
            <button className={styles.ghostBtn} onClick={onClose}>
              取消
            </button>
            <button
              className={styles.publishBtn}
              onClick={doPublish}
              disabled={publish.isPending}
              style={{ marginLeft: 0 }}
            >
              {publish.isPending ? "发布中…" : "发布"}
            </button>
          </div>
          {publish.isError && (
            <div className={styles.err}>{String(publish.error)}</div>
          )}
        </div>
      </div>
    </div>
  );
}
