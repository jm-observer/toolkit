/**
 * /english/all — 全量句子播放页。
 * 由 EnglishBootstrap 包裹，确保 g10_base + customerId 已加载。
 */

import EnglishBootstrap from './EnglishBootstrap'
import AnnotationPlayer from './components/AnnotationPlayer'

export default function EnglishAll() {
  return (
    <EnglishBootstrap>
      <AnnotationPlayer autoStart={true} dataSource="all" />
    </EnglishBootstrap>
  )
}
