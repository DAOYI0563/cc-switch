import { invoke } from "@tauri-apps/api/core";
import type { CliStatus } from "@/types";

export const cliStatusApi = {
  async getAll(): Promise<CliStatus[]> {
    return await invoke("get_cli_statuses");
  },
};
