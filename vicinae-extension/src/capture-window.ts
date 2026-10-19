import { closeMainWindow, showToast, Toast } from "@vicinae/api";
import { spawn } from "node:child_process";

export default async function Command() {
  launch("capture-window");
  await closeMainWindow();
}

function launch(command: string) {
  const child = spawn("/home/afridho/.local/bin/pureshot", [command], { detached: true, stdio: "ignore" });
  child.on("error", async (error) => {
    await showToast({ style: Toast.Style.Failure, title: "PureShot launch failed", message: error.message });
  });
  child.unref();
}
