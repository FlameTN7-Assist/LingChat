import { defineStore } from 'pinia'
import { getPlayerProfile, setPlayerProfile } from '@/api/services/player'
import type { PlayerProfile } from '@/api/services/game-info'

export const useUserStore = defineStore('user', {
  state: () => ({
    user_id: '1',
    client_id: '',
    /** 全局玩家档案（解耦玩家与 AI 设定） */
    playerProfile: {
      user_name: '玩家',
      user_subtitle: '',
      user_prompt: '',
    } as PlayerProfile,
    /** player_profile 是否已加载 */
    profileLoaded: false,
  }),
  getters: {
    /** 玩家名（快捷访问） */
    playerName: (state) => state.playerProfile.user_name,
    /** 玩家副标题 */
    playerSubtitle: (state) => state.playerProfile.user_subtitle,
    /** 玩家系统提示词 */
    playerPrompt: (state) => state.playerProfile.user_prompt,
  },
  actions: {
    /** 从后端加载玩家档案 */
    async loadPlayerProfile() {
      try {
        const profile = await getPlayerProfile()
        this.playerProfile = {
          user_name: profile.user_name || '玩家',
          user_subtitle: profile.user_subtitle || '',
          user_prompt: profile.user_prompt || '',
        }
        this.profileLoaded = true
      } catch (e) {
        console.warn('加载玩家档案失败:', e)
        this.profileLoaded = false
      }
    },

    /** 保存玩家档案 */
    async savePlayerProfile(profile: Partial<PlayerProfile>) {
      this.playerProfile = {
        ...this.playerProfile,
        ...profile,
      }
      try {
        await setPlayerProfile(
          this.playerProfile.user_name,
          this.playerProfile.user_subtitle,
          this.playerProfile.user_prompt,
        )
        return true
      } catch (e) {
        console.error('保存玩家档案失败:', e)
        return false
      }
    },

    /** 更新玩家名 */
    setPlayerName(name: string) {
      this.playerProfile.user_name = name
    },

    /** 更新玩家副标题 */
    setPlayerSubtitle(subtitle: string) {
      this.playerProfile.user_subtitle = subtitle
    },
  },
})
