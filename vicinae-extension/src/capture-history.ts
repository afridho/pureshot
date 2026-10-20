import { closeMainWindow, showToast, Toast } from "@vicinae/api";
import { spawn } from "node:child_process";

export default async function Command() {
  await closeMainWindow();
  const child = spawn("/home/afridho/.local/bin/pureshot", ["capture-history"], { detached: true, stdio: "ignore" });
  child.on("error", async (error) => {
    await showToast({ style: Toast.Style.Failure, title: "PureShot launch failed", message: error.message });
  });
  child.unref();
}
