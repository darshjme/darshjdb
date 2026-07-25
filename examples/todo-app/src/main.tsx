import React from "react";
import ReactDOM from "react-dom/client";
import { DarshanProvider } from "@darshjdb/react";
import { App } from "./App";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <DarshanProvider serverUrl="http://localhost:7700" appId="todo-example">
      <App />
    </DarshanProvider>
  </React.StrictMode>,
);
