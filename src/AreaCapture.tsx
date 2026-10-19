import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Point = { x: number; y: number };

export default function AreaCapture() {
  const [start, setStart] = useState<Point | null>(null);
  const [end, setEnd] = useState<Point | null>(null);
  const dragging = useRef(false);

  useEffect(() => {
    const cancel = (event: KeyboardEvent) => {
      if (event.key === "Escape") void invoke("cancel_area_capture");
    };
    window.addEventListener("keydown", cancel);
    return () => window.removeEventListener("keydown", cancel);
  }, []);

  const rect = start && end ? {
    left: Math.min(start.x, end.x), top: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x), height: Math.abs(end.y - start.y)
  } : null;

  return <main className="area-capture"
    onMouseDown={(e) => { dragging.current = true; setStart({x:e.clientX,y:e.clientY}); setEnd({x:e.clientX,y:e.clientY}); }}
    onMouseMove={(e) => { if (dragging.current) setEnd({x:e.clientX,y:e.clientY}); }}
    onMouseUp={(e) => {
      if (!dragging.current || !start) return;
      dragging.current = false;
      const endPoint = {x:e.clientX,y:e.clientY};
      setEnd(endPoint);
      const x = Math.min(start.x,endPoint.x), y = Math.min(start.y,endPoint.y);
      const width = Math.abs(endPoint.x-start.x), height = Math.abs(endPoint.y-start.y);
      if (width < 5 || height < 5) { setStart(null); setEnd(null); return; }
      void invoke("finish_area_capture", { x, y, width, height, scaleFactor: window.devicePixelRatio });
    }}>
    <div className="capture-help">Drag to capture an area <kbd>Esc</kbd> to cancel</div>
    {rect && <><div className="selection" style={rect}/><div className="selection-size" style={{left:rect.left,top:Math.max(8,rect.top-30)}}>{Math.round(rect.width * devicePixelRatio)} × {Math.round(rect.height * devicePixelRatio)}</div></>}
  </main>;
}
