import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

/**
 * 智能浮层（JS 实现，替代易出问题的 CSS 伪元素 data-tooltip 方案）。
 *
 * 为什么不用 `[data-tooltip]::after`：
 *  - CSS 伪元素浮层无法感知周边元素和视口边界，长内容在元素密集区必然遮挡/裁剪；
 *  - `max-width: min(…, calc(100vw - …))` 依赖 viewport 单位，在 WKWebView（Tauri）
 *    下解析异常会导致异常折行；
 *  - `transform: translateY(-50%)` + `transition: opacity` 在 WebKit 合成器下可能
 *    产生文字残影/重影。
 *
 * 本组件用 fixed 定位 + getBoundingClientRect 精确计算位置，宽度由 JS 限制在
 * 视口内，自动避让视口左右边缘，不依赖任何 viewport 单位与 transform 定位。
 */
export function Tooltip({
  content,
  children,
  placement = "right",
}: {
  content: ReactNode;
  children: ReactNode;
  placement?: "right" | "bottom";
}) {
  const anchorRef = useRef<HTMLSpanElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  const place = useCallback(() => {
    const anchor = anchorRef.current;
    const pop = popRef.current;
    if (!anchor || !pop) return;
    const rect = anchor.getBoundingClientRect();
    const margin = 8;
    const gap = 8;
    // 浮层最大宽度（纯 JS，规避 WKWebView 的 100vw 解析问题）：
    // right 贴边模式用满视口可用宽度；bottom 模式限制为较窄上限，长 URL/文本在此换行。
    const maxWidth =
      placement === "bottom"
        ? Math.min(320, Math.max(160, window.innerWidth - margin * 2))
        : Math.max(160, window.innerWidth - margin * 2);
    if (pop.style.maxWidth !== `${maxWidth}px`) {
      pop.style.maxWidth = `${maxWidth}px`;
    }
    const pw = pop.offsetWidth;
    const ph = pop.offsetHeight;

    let top: number;
    let left: number;
    if (placement === "bottom") {
      // 下方居中：水平相对触发元素居中并收拢进视口，避免左右溢出；
      // 下方空间不足时翻到上方。长文本由 .tooltip-pop 的 normal 换行承接。
      top = rect.bottom + gap;
      if (top + ph > window.innerHeight - margin) {
        top = rect.top - gap - ph;
        if (top < margin) top = margin;
      }
      left = rect.left + rect.width / 2 - pw / 2;
      left = Math.max(margin, Math.min(left, window.innerWidth - margin - pw));
    } else {
      // 优先放在触发元素右侧；右侧空间不足时放左侧；两侧都不足时贴边
      left = rect.right + gap;
      if (left + pw > window.innerWidth - margin) {
        left = rect.left - gap - pw;
        if (left < margin) {
          left = Math.max(margin, window.innerWidth - pw - margin);
        }
      }
      // 垂直居中于触发元素，超出视口上下边缘时收拢
      top = rect.top + rect.height / 2 - ph / 2;
      if (top < margin) top = margin;
      if (top + ph > window.innerHeight - margin) {
        top = Math.max(margin, window.innerHeight - ph - margin);
      }
    }
    setPos({ top, left });
  }, [placement]);

  useEffect(() => {
    if (!visible) return;
    place();
    // 浮层内容变化后重算一次（内容可能在渲染后才确定尺寸）
    const raf = requestAnimationFrame(place);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [visible, place]);

  return (
    <>
      <span
        ref={anchorRef}
        className="tooltip-anchor"
        onMouseEnter={() => setVisible(true)}
        onMouseLeave={() => setVisible(false)}
      >
        {children}
      </span>
      {visible &&
        createPortal(
          <div
            ref={popRef}
            className="tooltip-pop"
            style={{ top: pos?.top ?? 0, left: pos?.left ?? 0 }}
            role="tooltip"
          >
            {content}
          </div>,
          document.body,
        )}
    </>
  );
}
