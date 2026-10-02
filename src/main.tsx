import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { isMac } from "./platform";
import { applyTheme, savedTheme } from "./themes";

applyTheme(savedTheme());
if (isMac) document.documentElement.dataset.platform = "mac";

if (import.meta.env.VITE_E2E === "1") {
  await import("@wdio/tauri-plugin");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
