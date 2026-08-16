import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  skillsApi,
  type ImportSkillSelection,
  type InstalledSkill,
} from "@/lib/api/skills";
import type { ManagedAppId } from "@/lib/api/types";
import { mergeImportedSkills } from "@/hooks/useSkills.helpers";
import { runSequentialBulkAction } from "@/lib/utils/sequentialBulkAction";

export function useInstalledSkills() {
  return useQuery({
    queryKey: ["skills", "installed"],
    queryFn: () => skillsApi.getInstalled(),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
}

export function useUninstallSkill() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => skillsApi.uninstallUnified(id),
    onSuccess: (removed, id) => {
      if (!removed) return;
      queryClient.setQueryData<InstalledSkill[]>(
        ["skills", "installed"],
        (oldData) => oldData?.filter((skill) => skill.id !== id),
      );
    },
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
        queryClient.invalidateQueries({ queryKey: ["skills", "unmanaged"] }),
      ]),
  });
}

export function useToggleSkillApp() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      app,
      sourceApp,
      enabled,
    }: {
      id: string;
      app: ManagedAppId;
      sourceApp: ManagedAppId;
      enabled: boolean;
    }) => skillsApi.toggleApp(id, app, sourceApp, enabled),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
  });
}

export function useBulkToggleSkillApp() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      ids,
      app,
      sourceApps,
      enabled,
    }: {
      ids: string[];
      app: ManagedAppId;
      sourceApps: Record<string, ManagedAppId>;
      enabled: boolean;
    }) =>
      runSequentialBulkAction(ids, (id) =>
        skillsApi.toggleApp(id, app, sourceApps[id] ?? app, enabled),
      ),
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
  });
}

export function useSyncSkillFromLive() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, sourceApp }: { id: string; sourceApp: ManagedAppId }) =>
      skillsApi.syncFromLive(id, sourceApp),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
        queryClient.invalidateQueries({ queryKey: ["skills", "unmanaged"] }),
      ]),
  });
}

export function useScanUnmanagedSkills(options?: { enabled?: boolean }) {
  return useQuery({
    queryKey: ["skills", "unmanaged"],
    queryFn: () => skillsApi.scanUnmanaged(),
    enabled: options?.enabled ?? false,
    staleTime: 30 * 1000,
    placeholderData: keepPreviousData,
  });
}

export function useImportSkillsFromApps() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (imports: ImportSkillSelection[]) =>
      skillsApi.importFromApps(imports),
    onSuccess: (importedSkills) => {
      queryClient.setQueryData<InstalledSkill[]>(
        ["skills", "installed"],
        (oldData) => mergeImportedSkills(oldData, importedSkills),
      );
    },
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["skills", "installed"] }),
        queryClient.invalidateQueries({ queryKey: ["skills", "unmanaged"] }),
      ]),
  });
}

export type {
  ImportSkillSelection,
  InstalledSkill,
  UnmanagedSkill,
} from "@/lib/api/skills";
