import { applyTheme, loadThemePref } from "./theme";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ConfirmProvider } from "./components/ConfirmDialog";
import { ToastProvider } from "./components/Toast";
import "./styles.css";

applyTheme(loadThemePref());

class ErrorBoundary extends React.Component<{ children: React.ReactNode }, { err: Error | null }> {
  state: { err: Error | null } = { err: null };
  static getDerivedStateFromError(err: Error) {
    return { err };
  }
  render() {
    if (this.state.err) {
      return (
        <pre style={{ margin: 0, padding: 24, color: "#f07178", whiteSpace: "pre-wrap" }}>
          {this.state.err.stack || String(this.state.err)}
        </pre>
      );
    }
    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <ConfirmProvider>
        <ToastProvider>
          <App />
        </ToastProvider>
      </ConfirmProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);

