import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
// v0.2.5: side-effect import — boots i18next (loads zh-CN + en
// resources, configures localStorage persistence). After this returns
// every `useTranslation()` consumer renders with the active locale.
import "./i18n";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
