import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  title: string;
  stage: string;
  command: string;
}

export default function ModulePlaceholder({ title, stage, command }: Props) {
  const [pingResult, setPingResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function handlePing() {
    try {
      const result = await invoke<string>(command);
      setPingResult(result);
      setError(null);
    } catch (e) {
      setError(String(e));
      setPingResult(null);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <h1 className="text-xl font-semibold">{title}</h1>
      <p className="text-sm text-gray-500 dark:text-gray-400">
        TODO: 阶段 {stage} 迁入
      </p>
      <div className="flex items-center gap-3">
        <button
          onClick={handlePing}
          className="rounded-md bg-blue-600 px-4 py-1.5 text-sm text-white hover:bg-blue-700 active:bg-blue-800"
        >
          Ping 后端
        </button>
        {pingResult !== null && (
          <span className="text-sm text-green-600 dark:text-green-400">
            ✓ {pingResult}
          </span>
        )}
        {error !== null && (
          <span className="text-sm text-red-500">{error}</span>
        )}
      </div>
    </div>
  );
}
