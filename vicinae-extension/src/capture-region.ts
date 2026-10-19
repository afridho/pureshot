import { closeMainWindow, showToast, Toast } from "@vicinae/api";
import { spawn } from "node:child_process";

export default async function Command() {
  try {
    const child = spawn("/home/afridho/.local/bin/pureshot", ["capture-region"], { detached: true, stdio: "ignore" });
    child.on("error", async (error) => {
      await showToast({ style: Toast.Style.Failure, title: "PureShot launch failed", message: error.message });
    });
    child.unref();
    await closeMainWindow();
  } catch (error) {
    await showToast({ style: Toast.Style.Failure, title: "PureShot command failed", message: String(error) });
  }
}
