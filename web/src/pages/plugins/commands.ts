import { errorMessage, rpc } from '../../lib/rpc'
import { toast } from '../../ui'

type Translate = (key: string, params?: Record<string, string | number>) => string

/** 运行插件命令（`daemon.plugin.command`）；失败 toast `pluginCommandFailed`。 */
export async function runPluginCommand(t: Translate, identity: string, command: string, taskIds: string[] = []): Promise<void> {
  try {
    await rpc.daemon.plugin.command({ identity, command, taskIds })
  } catch (error) {
    toast.error(t('pluginCommandFailed', { message: errorMessage(error) }))
  }
}
