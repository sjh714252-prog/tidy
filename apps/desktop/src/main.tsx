import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ConfirmPopup } from "./views/ConfirmPopup";
import { MainWindow } from "./views/MainWindow";
import { SearchWindow } from "./views/SearchWindow";
import { WizardWindow } from "./views/WizardWindow";
import "./styles.css";

// One bundle, four windows (tauri.conf.json): the URL hash picks the view.
function App() {
  const route = window.location.hash.replace(/^#\/?/, "");
  if (route.startsWith("search")) return <SearchWindow />;
  if (route.startsWith("confirm")) return <ConfirmPopup />;
  if (route.startsWith("wizard")) return <WizardWindow />;
  return <MainWindow />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
