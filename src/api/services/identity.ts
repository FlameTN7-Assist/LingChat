import { invoke } from "@tauri-apps/api/core";

/** 实体人设扩展，对应 Rust `RoleProfile`（role.profile_json） */
export interface RoleProfile {
  subtitle: string;
  info: string;
  location_id: string | null;
  home_location_id: string | null;
  attributes: Record<string, unknown>;
}

/** 一条玩家身份（role_type=User 的统一实体） */
export interface IdentityInfo {
  role_id: number;
  name: string;
  profile: RoleProfile;
}

export const listIdentities = async (): Promise<IdentityInfo[]> => {
  try {
    return await invoke<IdentityInfo[]>("list_identities");
  } catch (error: any) {
    throw new Error(typeof error === "string" ? error : "获取玩家身份列表失败");
  }
};

export const createIdentity = async (name: string, profile: RoleProfile): Promise<number> => {
  try {
    return await invoke<number>("create_identity", { name, profile });
  } catch (error: any) {
    throw new Error(typeof error === "string" ? error : "创建玩家身份失败");
  }
};

export const updateIdentity = async (
  roleId: number,
  name: string,
  profile: RoleProfile,
): Promise<void> => {
  try {
    await invoke("update_identity", { roleId, name, profile });
  } catch (error: any) {
    throw new Error(typeof error === "string" ? error : "更新玩家身份失败");
  }
};

/** 删除玩家身份，返回是否实际删除（系统保护 id 会被拒绝并返回 false） */
export const deleteIdentity = async (roleId: number): Promise<boolean> => {
  try {
    return await invoke<boolean>("delete_identity", { roleId });
  } catch (error: any) {
    throw new Error(typeof error === "string" ? error : "删除玩家身份失败");
  }
};
