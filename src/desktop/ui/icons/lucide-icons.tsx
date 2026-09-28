import {
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Brain,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Columns2,
  CornerDownLeft,
  Cpu,
  Ellipsis,
  FileText,
  Folder,
  FolderPlus,
  Hash,
  Home,
  Inbox,
  Info,
  LayoutPanelLeft,
  List,
  Lock,
  Maximize2,
  Minimize2,
  Minus,
  Paintbrush,
  Palette,
  PanelLeft,
  PanelRight,
  Paperclip,
  PenLine,
  Plug,
  Plus,
  Rows2,
  Search,
  Settings2,
  Shield,
  ShieldAlert,
  ShieldCheck,
  SquarePen,
  SquareTerminal,
  X,
  Zap,
  type LucideIcon,
} from "lucide-react"
import type { DesktopIconComponent, DesktopIconFamily } from "./icon-contract"

/**
 * Every role drawn from Lucide, a third-party family, through the same
 * provider as Nessa's own — the proof that a family is only data. Lucide
 * keeps its own 24-unit artwork and stroke; the consumer still sizes and
 * colours it.
 */
function lucide(Icon: LucideIcon): DesktopIconComponent {
  return function LucideRoleIcon(props) {
    return <Icon {...props} />
  }
}

export const lucideIcons: DesktopIconFamily = {
  check: lucide(Check),
  close: lucide(X),
  chevronDown: lucide(ChevronDown),
  chevronUp: lucide(ChevronUp),
  chevronLeft: lucide(ChevronLeft),
  chevronRight: lucide(ChevronRight),
  moreHorizontal: lucide(Ellipsis),

  sidebar: lucide(PanelLeft),
  panelRight: lucide(PanelRight),
  sessionList: lucide(List),
  splitRight: lucide(Columns2),
  splitDown: lucide(Rows2),
  maximize: lucide(Maximize2),
  restore: lucide(Minimize2),
  back: lucide(ArrowLeft),
  forward: lucide(ArrowRight),
  home: lucide(Home),
  search: lucide(Search),

  newSession: lucide(SquarePen),
  add: lucide(Plus),
  channel: lucide(Hash),
  privateChannel: lucide(Lock),
  needsYou: lucide(ShieldAlert),
  running: lucide(Inbox),

  send: lucide(ArrowUp),
  attach: lucide(Paperclip),
  folder: lucide(Folder),
  folderAdd: lucide(FolderPlus),
  thinking: lucide(Brain),
  fast: lucide(Zap),
  access: lucide(Shield),
  enter: lucide(CornerDownLeft),

  customize: lucide(Paintbrush),
  zoomIn: lucide(Plus),
  zoomOut: lucide(Minus),

  file: lucide(FileText),
  edit: lucide(PenLine),
  terminal: lucide(SquareTerminal),

  preferences: lucide(Settings2),
  appearance: lucide(Palette),
  workspace: lucide(LayoutPanelLeft),
  model: lucide(Cpu),
  connections: lucide(Plug),
  privacy: lucide(ShieldCheck),
  about: lucide(Info),
}
