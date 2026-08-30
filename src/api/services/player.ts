import { invoke } from '@tauri-apps/api/core'
import type { PlayerProfile } from './game-info'

/**
 * 读取全局玩家档案。
 */
export const getPlayerProfile = async (): Promise<PlayerProfile> => {
  return await invoke<PlayerProfile>('get_player_profile')
}

/**
 * 保存全局玩家档案。
 */
export const setPlayerProfile = async (
  user_name: string,
  user_subtitle?: string,
  user_prompt?: string,
): Promise<{ success: boolean }> => {
  return await invoke<{ success: boolean }>('set_player_profile', {
    userName: user_name,
    userSubtitle: user_subtitle,
    userPrompt: user_prompt,
  })
}
