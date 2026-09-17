import { useState } from "react";
import {
  is_webgl2_supported,
} from "vm-rust";
import { useVMHandle } from "../../components/VMProvider";
import styles from "./styles.module.css";

const STORAGE_KEY_BACKEND = "dirplayer_renderer_backend";
const STORAGE_KEY_PFR = "dirplayer_pfr_enabled";

export default function RenderingOptions() {
  const handle = useVMHandle();
  const [backend, setBackend] = useState(() => handle.renderer_backend());
  const [pfrEnabled, setPfrEnabled] = useState(() => handle.pfr_font_enabled());
  const webgl2Supported = is_webgl2_supported();

  const handleBackendChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const value = e.target.value;
    try {
      handle.set_renderer_backend(value);
      const actual = handle.renderer_backend();
      setBackend(actual);
      localStorage.setItem(STORAGE_KEY_BACKEND, actual);
    } catch (err) {
      console.error("Failed to switch renderer:", err);
    }
  };

  const handlePfrToggle = (e: React.ChangeEvent<HTMLInputElement>) => {
    const enabled = e.target.checked;
    handle.set_pfr_font_enabled(enabled);
    setPfrEnabled(enabled);
    localStorage.setItem(STORAGE_KEY_PFR, String(enabled));
  };

  return (
    <div className={styles.optionsContainer}>
      <div className={styles.optionRow}>
        <label htmlFor="renderer-backend">Renderer</label>
        <select
          id="renderer-backend"
          value={backend}
          onChange={handleBackendChange}
        >
          <option value="Canvas2D">Canvas 2D</option>
          <option value="WebGL2" disabled={!webgl2Supported}>
            WebGL2{!webgl2Supported ? " (not supported)" : ""}
          </option>
        </select>
      </div>
      <div className={styles.optionRow}>
        <label htmlFor="pfr-font-toggle">PFR font rendering</label>
        <input
          id="pfr-font-toggle"
          type="checkbox"
          checked={pfrEnabled}
          onChange={handlePfrToggle}
        />
      </div>
    </div>
  );
}
