import { render } from "@solidjs/web";
import { App } from "./app";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");

render(() => <App />, root);
