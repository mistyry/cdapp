import { Copy, Minus, Square, X } from "lucide-solid";
import { closeWindow, maximized, minimize, toggleMaximize } from "../../stores/window";

export default function WindowControls() {
  return (
    <div class="win-ctrls">
      <button class="win-btn" aria-label="最小化" onClick={minimize}><Minus size={16} stroke-width={1.5} /></button>
      <button class="win-btn" aria-label={maximized() ? "还原" : "最大化"} onClick={toggleMaximize}>
        {maximized() ? <Copy size={14} stroke-width={1.5} /> : <Square size={13} stroke-width={1.5} />}
      </button>
      <button class="win-btn close" aria-label="关闭" onClick={closeWindow}><X size={16} stroke-width={1.5} /></button>
    </div>
  );
}
