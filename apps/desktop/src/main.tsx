import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ConfirmPopup } from "./views/ConfirmPopup";
import { MainWindow } from "./views/MainWindow";
import { SearchWindow } from "./views/SearchWindow";
import "./styles.css";

// One bundle, three windows (tauri.conf.json): the URL hash picks the view.
function App() {
  const route = window.location.hash.replace(/^#\/?/, "");
  if (route.startsWith("search")) return <SearchWindow />;
  if (route.startsWith("confirm")) return <ConfirmPopup />;
  return <MainWindow />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
