import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { ExplainProvider } from "./explain";
import "./styles.css";

// The explanation layer wraps everything, because there is no screen that does
// not have something on it worth explaining.
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ExplainProvider>
      <App />
    </ExplainProvider>
  </React.StrictMode>,
);
