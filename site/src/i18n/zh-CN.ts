// 页面文案，简体中文。结构必须和 en.ts 一致（类型会检查）。
// 产品名不翻译：Pocket Hub、Pocket Applets、Pocket Nexus。
// Applet 也不翻译，和商店网站一致。
import type { Wording } from "./index.ts";

const zhCN: Wording = {
  title: "Pocket Hub",
  language: "语言",
  header: {
    home: "Pocket Hub",
    store: "Applet 商店",
  },
  lede: "把 Pocket Applet 装到你的设备上，并让它们保持最新。",
  release: {
    facts: "v{version} · {size}",
    notes: "发行说明",
    checksum: "SHA-256",
  },
  platforms: {
    macos: {
      download: "下载 macOS 版",
      requires: "Apple 芯片 · macOS 15 或更高版本",
    },
  },
  devices: {
    title: "支持的设备",
  },
  footer: {
    credit: "© 2026 {nexus}。自由软件，以 GPL-3.0 许可发布。",
    source: "源代码",
  },
};

export default zhCN;
