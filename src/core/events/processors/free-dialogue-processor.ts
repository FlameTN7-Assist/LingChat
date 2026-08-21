import type { IEventProcessor } from '../event-processor'
import { WebSocketMessageTypes, type ScriptFreeDialogueEvent } from '../../../types'
import { useGameStore } from '../../../stores/modules/game'

export default class FreeDialogueProcessor implements IEventProcessor {
  canHandle(eventType: string): boolean {
    return eventType === WebSocketMessageTypes.SCRIPT_FREE_DIALOGUE
  }

  async processEvent(event: ScriptFreeDialogueEvent): Promise<void> {
    const gameStore = useGameStore()

    // 处理对话逻辑
    if (!gameStore.runningScript) return

    const freeDialogue = gameStore.runningScript.freeDialogueInfo

    freeDialogue.isFreeDialogue = event.switch

    if (freeDialogue.isFreeDialogue) {
      freeDialogue.maxRounds = event.maxRounds
      freeDialogue.endLine = event.endLine
      // 读档续轮：同步后端已进行的轮次（saved_rounds），避免续跑后前端提示
      // 停留在 0/N 而后端已到第 N 轮。正常新跑时后端给 0。
      freeDialogue.currentRound = event.currentRound ?? 0
    } else {
      freeDialogue.currentRound = 0
      freeDialogue.maxRounds = 0
      freeDialogue.endLine = ''
    }
  }
}
