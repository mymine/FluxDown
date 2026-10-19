// 节点 kind → 组件。未知 kind 渲染为空（daemon 已按 schema 拒绝，防御用）。

import type { ReactNode } from 'react'
import type { ViewNode } from '../../../../lib/rpc'
import { BadgeNode, CodeNode, HeadingNode, ImageNode, LinkNode, MarkdownNode, ProgressNode, SpinnerNode, TextNode } from './content'
import { ButtonNode, CheckboxNode, InputNode, SelectNode, SwitchNode, TabsNode, TextareaNode } from './controls'
import { ListNode, TableNode } from './data'
import { CalloutNode, CardNode, DividerNode, EmptyNode, KvNode, ScrollNode, SectionNode, StackNode } from './layout'

function renderChildren(node: ViewNode): ReactNode {
  return node.c.map((child) => <ViewNodeView key={child.id} node={child} />)
}

export function ViewNodeView({ node }: { node: ViewNode }): ReactNode {
  switch (node.k) {
    case 'stack':
      return <StackNode node={node}>{renderChildren(node)}</StackNode>
    case 'scroll':
      return <ScrollNode node={node}>{renderChildren(node)}</ScrollNode>
    case 'card':
      return <CardNode node={node}>{renderChildren(node)}</CardNode>
    case 'section':
      return <SectionNode node={node}>{renderChildren(node)}</SectionNode>
    case 'divider':
      return <DividerNode />
    case 'text':
      return <TextNode node={node} />
    case 'heading':
      return <HeadingNode node={node} />
    case 'markdown':
      return <MarkdownNode node={node} />
    case 'code':
      return <CodeNode node={node} />
    case 'link':
      return <LinkNode node={node} />
    case 'image':
      return <ImageNode node={node} />
    case 'badge':
      return <BadgeNode node={node} />
    case 'progress':
      return <ProgressNode node={node} />
    case 'spinner':
      return <SpinnerNode node={node} />
    case 'kv':
      return <KvNode node={node} />
    case 'callout':
      return <CalloutNode node={node} />
    case 'empty':
      return <EmptyNode node={node} />
    case 'button':
      return <ButtonNode node={node} />
    case 'switch':
      return <SwitchNode node={node} />
    case 'checkbox':
      return <CheckboxNode node={node} />
    case 'select':
      return <SelectNode node={node} />
    case 'input':
      return <InputNode node={node} />
    case 'textarea':
      return <TextareaNode node={node} />
    case 'tabs':
      return <TabsNode node={node} />
    case 'table':
      return <TableNode node={node} />
    case 'list':
      return <ListNode node={node} />
    default:
      return null
  }
}
