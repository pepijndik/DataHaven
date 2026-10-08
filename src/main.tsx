import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

async function start() {
  // Design the UI in a browser without the desktop backend: open the dev server with ?mock.
  if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("mock")) {
    window.backupApp = (await import("./devMockApi")).devMockApi;
  }
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void start();
