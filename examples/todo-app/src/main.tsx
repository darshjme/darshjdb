import React from "react";
import ReactDOM from "react-dom/client";
import { DarshanProvider, DarshanDB } from "@darshan/react";
import { App } from "./App";

const db = DarshanDB.init({ url: "http://localhost:7700" });

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <DarshanProvider db={db}>
      <App />
    </DarshanProvider>
  </React.StrictMode>,
);
