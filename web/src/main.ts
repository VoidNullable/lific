import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { init as initTheme } from "./lib/theme";

initTheme();

const app = mount(App, {
  target: document.getElementById("app")!,
});

export default app;
