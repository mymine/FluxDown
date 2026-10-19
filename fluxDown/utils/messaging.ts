/**
 * 扩展内部消息的类型化发送端（契约见 background/protocol.ts 的 MessageMap）。
 * content script、popup、options、sidepanel 共用；background 侧路由在 background/messages.ts。
 */
import { browser } from "wxt/browser";
import type { MessageAction, MessageRequest, MessageResponse } from "@/background/protocol";

export async function sendAction<A extends MessageAction>(
  message: MessageRequest<A>,
): Promise<MessageResponse<A>> {
  return (await browser.runtime.sendMessage(message)) as MessageResponse<A>;
}
