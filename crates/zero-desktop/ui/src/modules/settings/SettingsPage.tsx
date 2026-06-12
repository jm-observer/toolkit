import { useEffect, useState } from "react";

function getStoredTheme(): "light" | "dark" {
  return (localStorage.getItem("theme") as "light" | "dark") ?? "light";
}

function applyTheme(theme: "light" | "dark") {
  if (theme === "dark") {
    document.documentElement.classList.add("dark");
  } else {
    document.documentElement.classList.remove("dark");
  }
  localStorage.setItem("theme", theme);
}

export default function SettingsPage() {
  const [theme, setTheme] = useState<"light" | "dark">(getStoredTheme);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  function toggleTheme() {
    setTheme((prev) => (prev === "light" ? "dark" : "light"));
  }

  return (
    <div className="flex flex-col gap-6">
      <h1 className="text-xl font-semibold">设置</h1>

      <section className="flex flex-col gap-3">
        <h2 className="text-sm font-medium text-gray-600 dark:text-gray-400">
          外观
        </h2>
        <div className="flex items-center gap-3">
          <span className="text-sm">主题</span>
          <button
            onClick={toggleTheme}
            className="rounded-md border border-gray-300 px-4 py-1.5 text-sm hover:bg-gray-100 dark:border-gray-600 dark:hover:bg-gray-800"
          >
            {theme === "light" ? "切换深色" : "切换浅色"}
          </button>
          <span className="text-xs text-gray-400">当前：{theme === "light" ? "浅色" : "深色"}</span>
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-medium text-gray-600 dark:text-gray-400">
          TODO（后续阶段）
        </h2>
        <ul className="list-disc pl-5 text-sm text-gray-500 dark:text-gray-400">
          <li>G10 base URL 配置</li>
          <li>语音识别端点配置</li>
          <li>customer_id 配置</li>
          <li>日志目录查看</li>
        </ul>
      </section>
    </div>
  );
}
