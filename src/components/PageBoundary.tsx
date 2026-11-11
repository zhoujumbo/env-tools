import React from "react";

/**
 * 每页一层错误边界：某个页面渲染炸了只坏这一页，
 * 不至于整窗白屏（配合 Rust 侧 lock 容错，避免用户以为程序挂了）。
 */
export default class PageBoundary extends React.Component<
  { children: React.ReactNode },
  { err: Error | null }
> {
  state: { err: Error | null } = { err: null };

  static getDerivedStateFromError(err: Error) {
    return { err };
  }

  render() {
    if (this.state.err) {
      return (
        <div className="col">
          <div className="card">
            <h3>这个页面出错了</h3>
            <p className="muted">其它页面仍可正常使用。可以先重试，或切到别的页再回来。</p>
            <pre className="mono log-body">{this.state.err.stack || String(this.state.err)}</pre>
            <button className="btn primary" onClick={() => this.setState({ err: null })}>
              重试
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
