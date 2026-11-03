/** Image preview for a diff. Commit clicks otherwise land on the binary notice. */
import { useEffect, useState } from "react";
import { call, commands } from "@/platform/ipc";
import { Spinner } from "@/ui";
import { isImagePath } from "@/features/viewers/fileKinds";
import { sideFile, type DiffSide } from "./diffInput";
import styles from "./DiffEditor.module.css";

export function diffShowsImage(path: string, left: DiffSide, right: DiffSide): boolean {
  return (
    isImagePath(path) ||
    (left.type === "file" && isImagePath(left.path)) ||
    (right.type === "file" && isImagePath(right.path))
  );
}

async function sideImage(path: string, side: DiffSide): Promise<string | null> {
  switch (side.type) {
    case "worktree":
      return isImagePath(path) ? call(commands.fsReadImage(path)) : null;
    case "file":
      return isImagePath(side.path) ? call(commands.fsReadImage(side.path)) : null;
    case "rev": {
      const file = side.path ?? path;
      return isImagePath(file) ? call(commands.vcsReadImage(file, side.rev)) : null;
    }
    case "text":
    case "history":
      return null;
  }
}

function fileName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function ImagePane({ url, name }: { url: string | null; name: string }) {
  const [dim, setDim] = useState<{ w: number; h: number } | null>(null);
  if (!url) return <div className={styles.imagePane} />;
  // Icons are a few pixels; scale them up the way the file viewer does, without blurring.
  const scale = dim ? Math.min(8, Math.max(1, Math.floor(160 / Math.max(dim.w, dim.h, 1)))) : 1;
  return (
    <div className={styles.imagePane}>
      <img
        src={url}
        alt={name}
        className={styles.imagePreview}
        style={dim ? { width: dim.w * scale, height: dim.h * scale } : undefined}
        onLoad={(e) => setDim({ w: e.currentTarget.naturalWidth || 16, h: e.currentTarget.naturalHeight || 16 })}
      />
    </div>
  );
}

export function ImageDiff({ path, left, right }: { path: string; left: DiffSide; right: DiffSide }) {
  const leftKey = sideFile(left, path) ?? (left.type === "rev" ? `${left.rev}:${left.path ?? ""}` : left.type);
  const rightKey = sideFile(right, path) ?? (right.type === "rev" ? `${right.rev}:${right.path ?? ""}` : right.type);
  const request = `${path}\0${leftKey}\0${rightKey}`;
  const [loaded, setLoaded] = useState<{
    request: string;
    urls: { a: string | null; b: string | null } | null;
    error: string | null;
  } | null>(null);
  const current = loaded?.request === request ? loaded : null;

  useEffect(() => {
    let live = true;
    Promise.all([sideImage(path, left), sideImage(path, right)])
      .then(([a, b]) => {
        if (live) setLoaded({ request, urls: { a, b }, error: null });
      })
      .catch((e: unknown) => {
        if (live) setLoaded({ request, urls: null, error: e instanceof Error ? e.message : String(e) });
      });
    return () => {
      live = false;
    };
  }, [path, left, right, request]);

  if (current?.error) return <div className={styles.message}>{current.error}</div>;
  if (!current?.urls) {
    return (
      <div className={styles.message}>
        <Spinner size={14} />
      </div>
    );
  }
  const name = fileName(path);
  return (
    <div className={styles.images} role="region" aria-label={name}>
      <ImagePane url={current.urls.a} name={name} />
      <ImagePane url={current.urls.b} name={name} />
    </div>
  );
}
