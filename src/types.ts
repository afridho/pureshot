export type CaptureMode = "area" | "window" | "fullscreen";

export interface CaptureItem {
  id: string;
  path: string;
  dataUrl: string;
  width: number;
  height: number;
  createdAt: number;
}
