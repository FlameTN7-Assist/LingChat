<template>
  <MenuItem :title="$t('settings.playerProfile.title')">
    <template #header>
      <UserRound :size="20" />
    </template>
    <div class="space-y-3 p-3 w-full">
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div class="space-y-1.5">
          <label class="text-xs text-white/60 font-medium">{{ $t('settings.playerProfile.userName') }}</label>
          <input
            v-model="localProfile.user_name"
            type="text"
            :placeholder="$t('settings.playerProfile.userNamePlaceholder')"
            class="w-full rounded-xl bg-white/10 border border-white/20 px-3 py-2 text-sm text-white focus:outline-none focus:border-amber-300/70 transition"
          />
        </div>
        <div class="space-y-1.5">
          <label class="text-xs text-white/60 font-medium">{{ $t('settings.playerProfile.userSubtitle') }}</label>
          <input
            v-model="localProfile.user_subtitle"
            type="text"
            :placeholder="$t('settings.playerProfile.userSubtitlePlaceholder')"
            class="w-full rounded-xl bg-white/10 border border-white/20 px-3 py-2 text-sm text-white focus:outline-none focus:border-amber-300/70 transition"
          />
        </div>
      </div>
      <div class="flex justify-end gap-2">
        <Button type="big" :disabled="saving" @click="handleSave">
          {{ $t('settings.playerProfile.save') }}
        </Button>
      </div>
      <p v-if="savedTip" class="text-xs text-emerald-400/80">{{ savedTip }}</p>
    </div>
  </MenuItem>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { UserRound } from 'lucide-vue-next'
import { MenuItem } from '../../ui'
import { Button } from '../../base'
import { useUserStore } from '../../../stores/modules/user/user'
import type { PlayerProfile } from '../../../api/services/game-info'

const { t } = useI18n()
const userStore = useUserStore()

const localProfile = ref<PlayerProfile>({
  user_name: '玩家',
  user_subtitle: '',
  user_prompt: '',
})
const saving = ref(false)
const savedTip = ref('')

// 从 user store 加载初始值
async function loadProfile() {
  await userStore.loadPlayerProfile()
  localProfile.value = { ...userStore.playerProfile }
}

// 保存
async function handleSave() {
  saving.value = true
  try {
    const ok = await userStore.savePlayerProfile({
      user_name: localProfile.value.user_name.trim() || '玩家',
      user_subtitle: localProfile.value.user_subtitle,
      user_prompt: localProfile.value.user_prompt,
    })
    if (ok) {
      savedTip.value = t('settings.playerProfile.saved')
      setTimeout(() => (savedTip.value = ''), 2000)
    }
  } finally {
    saving.value = false
  }
}

onMounted(loadProfile)

// 当 user store 更新时同步展示（如初始化数据已加载）
watch(
  () => userStore.playerProfile,
  (profile) => {
    if (!saving.value) {
      localProfile.value = { ...profile }
    }
  },
  { deep: true },
)
</script>
