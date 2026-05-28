import { useEffect, useRef } from "react";

interface Props {
  /** Called continuously during drag with the current desired width (px) of
   *  the LEFT pane the splitter is attached to. */
  onResize: (newLeftPaneWidthPx: number) => void;
  /** Element used as the reference for measuring drag (typically the left
   *  pane wrapper). We compute new width = mouseX - leftPaneRect.left. */
  leftPaneRef: React.RefObject<HTMLElement | null>;
  minPx?: number;
  maxPx?: number;
}

export function Splitter({ onResize, leftPaneRef, minPx = 200, maxPx = 1200 }: Props) {
  const dragging = useRef(false);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      const rect = leftPaneRef.current?.getBoundingClientRect();
      if (!rect) return;
      const w = Math.max(minPx, Math.min(maxPx, e.clientX - rect.left));
      onResize(w);
    };
    const onUp = () => {
      if (!dragging.current) return;
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [onResize, leftPaneRef, minPx, maxPx]);

  return (
    <div
      className="splitter"
      onMouseDown={(e) => {
        e.preventDefault();
        dragging.current = true;
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
      }}
    />
  );
}
