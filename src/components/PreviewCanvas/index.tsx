import { useEffect, useState } from "react";
import { useVMHandle } from "../VMProvider";

interface PreviewCanvasProps {
  fontSize?: number;
}

export default function PreviewCanvas({ fontSize }: PreviewCanvasProps) {
  const handle = useVMHandle();
  const [parent, setParent] = useState<HTMLDivElement | null>(null);
  const onBitmapPreviewRef = (ref: HTMLDivElement | null) => {
    setParent(ref);
  };
  useEffect(() => {
    handle.set_preview_parent(parent ?? undefined);
    return () => {
      handle.set_preview_parent(undefined);
    };
  }, [handle, parent]);

  useEffect(() => {
    handle.set_preview_font_size(fontSize ?? 0);
  }, [handle, fontSize]);

  return <div id="bitmapPreview" ref={onBitmapPreviewRef}></div>;
}
