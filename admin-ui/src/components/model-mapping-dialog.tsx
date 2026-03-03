import { useState, useEffect } from 'react'
import { toast } from 'sonner'
import { Plus, Trash2 } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useModelMapping, useSetModelMapping } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'

interface ModelMappingDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

interface MappingEntry {
  source: string
  target: string
}

export function ModelMappingDialog({ open, onOpenChange }: ModelMappingDialogProps) {
  const [entries, setEntries] = useState<MappingEntry[]>([])
  const { data: mappingData, isLoading } = useModelMapping()
  const { mutate: saveMapping, isPending: isSaving } = useSetModelMapping()

  // 当对话框打开时，从服务器数据初始化
  useEffect(() => {
    if (open && mappingData?.mapping) {
      const loaded = Object.entries(mappingData.mapping).map(([source, target]) => ({
        source,
        target,
      }))
      setEntries(loaded.length > 0 ? loaded : [{ source: '', target: '' }])
    } else if (open) {
      setEntries([{ source: '', target: '' }])
    }
  }, [open, mappingData])

  const addEntry = () => {
    setEntries([...entries, { source: '', target: '' }])
  }

  const removeEntry = (index: number) => {
    setEntries(entries.filter((_, i) => i !== index))
  }

  const updateEntry = (index: number, field: 'source' | 'target', value: string) => {
    const updated = [...entries]
    updated[index] = { ...updated[index], [field]: value }
    setEntries(updated)
  }

  const handleSave = () => {
    // 过滤掉空行，构建映射
    const mapping: Record<string, string> = {}
    for (const entry of entries) {
      const source = entry.source.trim()
      const target = entry.target.trim()
      if (source && target) {
        mapping[source] = target
      }
    }

    saveMapping(mapping, {
      onSuccess: () => {
        toast.success('模型映射已保存')
        onOpenChange(false)
      },
      onError: (error) => {
        toast.error(`保存失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>模型映射</DialogTitle>
        </DialogHeader>

        <div className="space-y-3 py-4">
          <p className="text-sm text-muted-foreground">
            将实际模型名映射为显示模型名。客户端将看到映射后的模型名。
          </p>

          {isLoading ? (
            <div className="text-center py-4 text-muted-foreground">加载中...</div>
          ) : (
            <div className="space-y-2">
              {/* 表头 */}
              <div className="grid grid-cols-[1fr_auto_1fr_auto] gap-2 items-center">
                <span className="text-xs text-muted-foreground font-medium">实际模型</span>
                <span />
                <span className="text-xs text-muted-foreground font-medium">显示为</span>
                <span />
              </div>

              {entries.map((entry, index) => (
                <div key={index} className="grid grid-cols-[1fr_auto_1fr_auto] gap-2 items-center">
                  <Input
                    placeholder="claude-sonnet-4-5-20250929"
                    value={entry.source}
                    onChange={(e) => updateEntry(index, 'source', e.target.value)}
                    className="text-sm"
                  />
                  <span className="text-muted-foreground text-sm px-1">&rarr;</span>
                  <Input
                    placeholder="claude-opus-4-6"
                    value={entry.target}
                    onChange={(e) => updateEntry(index, 'target', e.target.value)}
                    className="text-sm"
                  />
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 text-muted-foreground hover:text-destructive"
                    onClick={() => removeEntry(index)}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              ))}

              <Button
                variant="outline"
                size="sm"
                onClick={addEntry}
                className="w-full"
              >
                <Plus className="h-4 w-4 mr-2" />
                添加映射
              </Button>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={isSaving || isLoading}>
            {isSaving ? '保存中...' : '保存'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
