import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useQueryClient } from '@tanstack/react-query'
import { Copy, ExternalLink, Loader2, CheckCircle2, XCircle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  cancelBuilderIdLogin,
  pollBuilderIdLogin,
  startBuilderIdLogin,
} from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { StartBuilderIdRequest, StartBuilderIdResponse } from '@/types/api'

interface BuilderIdLoginPanelProps {
  metadata: StartBuilderIdRequest
  onSuccess: (credentialId: number, email?: string) => void
  onCancel: () => void
}

type Phase = 'idle' | 'starting' | 'waiting' | 'error' | 'success'

export function BuilderIdLoginPanel({
  metadata,
  onSuccess,
  onCancel,
}: BuilderIdLoginPanelProps) {
  const [phase, setPhase] = useState<Phase>('idle')
  const [session, setSession] = useState<StartBuilderIdResponse | null>(null)
  const [errorMsg, setErrorMsg] = useState<string>('')
  const [secondsLeft, setSecondsLeft] = useState<number>(0)
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const countdownTimerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const queryClient = useQueryClient()

  const clearTimers = () => {
    if (pollTimerRef.current) {
      clearTimeout(pollTimerRef.current)
      pollTimerRef.current = null
    }
    if (countdownTimerRef.current) {
      clearInterval(countdownTimerRef.current)
      countdownTimerRef.current = null
    }
  }

  useEffect(() => {
    return () => {
      clearTimers()
    }
  }, [])

  const schedulePoll = (loginId: number, intervalSecs: number) => {
    clearTimers()
    pollTimerRef.current = setTimeout(() => {
      runPoll(loginId)
    }, intervalSecs * 1000)
  }

  const runPoll = async (loginId: number) => {
    try {
      const result = await pollBuilderIdLogin(loginId)
      switch (result.status) {
        case 'pending':
          schedulePoll(loginId, result.interval)
          break
        case 'slow_down':
          schedulePoll(loginId, result.interval)
          break
        case 'success':
          clearTimers()
          setPhase('success')
          queryClient.invalidateQueries({ queryKey: ['credentials'] })
          toast.success(result.message)
          onSuccess(result.credentialId, result.email)
          break
        case 'expired':
          clearTimers()
          setPhase('error')
          setErrorMsg(result.message)
          break
        case 'denied':
          clearTimers()
          setPhase('error')
          setErrorMsg(result.message)
          break
        case 'error':
          clearTimers()
          setPhase('error')
          setErrorMsg(result.message)
          break
      }
    } catch (e) {
      clearTimers()
      setPhase('error')
      setErrorMsg(extractErrorMessage(e))
    }
  }

  const handleStart = async () => {
    setPhase('starting')
    setErrorMsg('')
    try {
      const resp = await startBuilderIdLogin(metadata)
      setSession(resp)
      setPhase('waiting')
      setSecondsLeft(resp.expiresIn)

      countdownTimerRef.current = setInterval(() => {
        setSecondsLeft((s) => (s > 0 ? s - 1 : 0))
      }, 1000)
      schedulePoll(resp.loginId, resp.interval)
    } catch (e) {
      setPhase('error')
      setErrorMsg(extractErrorMessage(e))
    }
  }

  const handleCancel = async () => {
    clearTimers()
    const loginId = session?.loginId
    setPhase('idle')
    setSession(null)
    if (loginId) {
      try {
        await cancelBuilderIdLogin(loginId)
      } catch {
        // 忽略取消失败
      }
    }
    onCancel()
  }

  const handleRetry = () => {
    setErrorMsg('')
    setSession(null)
    setPhase('idle')
  }

  const copyUserCode = async () => {
    if (!session) return
    try {
      await navigator.clipboard.writeText(session.userCode)
      toast.success('用户码已复制到剪贴板')
    } catch {
      toast.error('复制失败，请手动复制')
    }
  }

  if (phase === 'idle') {
    return (
      <div className="space-y-3 rounded-md border border-dashed p-4">
        <p className="text-sm text-muted-foreground">
          点击下方按钮发起 Builder ID 登录。服务端会生成一次性用户码，并引导你在浏览器中完成授权；
          完成后 refreshToken 会自动写入凭据列表。
        </p>
        <Button type="button" onClick={handleStart}>
          开始登录
        </Button>
      </div>
    )
  }

  if (phase === 'starting') {
    return (
      <div className="flex items-center gap-2 rounded-md border p-4 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        正在向 AWS OIDC 申请设备码...
      </div>
    )
  }

  if (phase === 'error') {
    return (
      <div className="space-y-3 rounded-md border border-destructive/50 bg-destructive/5 p-4">
        <div className="flex items-start gap-2 text-sm text-destructive">
          <XCircle className="mt-0.5 h-4 w-4 shrink-0" />
          <span className="break-words">{errorMsg || '登录失败'}</span>
        </div>
        <div className="flex gap-2">
          <Button type="button" variant="outline" size="sm" onClick={handleRetry}>
            重新登录
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
            返回
          </Button>
        </div>
      </div>
    )
  }

  if (phase === 'success') {
    return (
      <div className="flex items-center gap-2 rounded-md border border-green-500/40 bg-green-500/5 p-4 text-sm">
        <CheckCircle2 className="h-4 w-4 text-green-600" />
        <span>登录成功，正在关闭对话框...</span>
      </div>
    )
  }

  // waiting
  const mm = Math.floor(secondsLeft / 60)
    .toString()
    .padStart(2, '0')
  const ss = (secondsLeft % 60).toString().padStart(2, '0')

  return (
    <div className="space-y-4 rounded-md border p-4">
      <div className="space-y-1">
        <div className="text-xs text-muted-foreground">用户码（User Code）</div>
        <div className="flex items-center gap-2">
          <code className="select-all rounded bg-muted px-3 py-2 font-mono text-lg tracking-widest">
            {session?.userCode}
          </code>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={copyUserCode}
            title="复制用户码"
          >
            <Copy className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div className="space-y-1">
        <div className="text-xs text-muted-foreground">授权链接</div>
        <a
          href={session?.verificationUriComplete}
          target="_blank"
          rel="noreferrer"
          className="inline-flex items-center gap-1.5 break-all text-sm text-primary underline-offset-2 hover:underline"
        >
          <ExternalLink className="h-3.5 w-3.5 shrink-0" />
          {session?.verificationUriComplete}
        </a>
        <p className="text-xs text-muted-foreground">
          在新打开的页面点击 “Confirm and continue” / “Allow” 完成授权
        </p>
      </div>

      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span className="flex items-center gap-1.5">
          <Loader2 className="h-3 w-3 animate-spin" />
          等待浏览器授权...
        </span>
        <span>
          剩余 <span className="font-mono">{mm}:{ss}</span>
        </span>
      </div>

      <div className="flex justify-end">
        <Button type="button" variant="outline" size="sm" onClick={handleCancel}>
          取消登录
        </Button>
      </div>
    </div>
  )
}
