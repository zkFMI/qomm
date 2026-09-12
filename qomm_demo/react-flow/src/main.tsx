import { memo, useCallback, useEffect, useMemo, useRef, type KeyboardEvent } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeMouseHandler,
  type NodeProps,
  getViewportForBounds,
  useReactFlow,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import './network.css';

type GraphMetric = {
  label: string;
  value: string;
  fullLabel?: string;
  fullValue?: string;
  tone?: 'normal' | 'reserved' | 'muted';
};

type GraphNode = {
  id: string;
  type: string;
  x: number;
  y: number;
  w: number;
  h: number;
  title: string;
  sub?: string;
  badge?: string;
  classes?: string[];
  metrics?: GraphMetric[];
};

type GraphEdge = {
  id: string;
  source: string;
  target: string;
  d: string;
  state: 'idle' | 'flow' | 'done' | 'cut';
  color: 'teal' | 'amber' | 'blue';
  own: boolean;
  particle?: boolean;
};

type GraphLabel = {
  x: number;
  y: number;
  text: string;
  strong?: boolean;
};

export type QommGraphModel = {
  nodes: GraphNode[];
  edges: GraphEdge[];
  labels: GraphLabel[];
  W: number;
  H: number;
};

// The page (demo.js) owns every string and every piece of chrome around the
// canvas: phase strip, legend, notes and the drawer that opens when a card is
// selected. The bundle draws the cards and the edges, reports which card was
// chosen, and keeps the viewport sensible for the container it was given.
export type QommGraphOptions = {
  ariaLabel: string;
  phase: string;
  phaseLabel: string;
  noRoundText: string;
  legend: Array<{ type: string; label: string }>;
  legendNotes: string[];
  reducedMotion: boolean;
  /** Narrow layout: fit the model to the canvas width and start at the top;
   *  the person pans down through the phases instead of reading a miniature. */
  compact?: boolean;
  /** The card whose drawer is open, highlighted and announced as expanded. */
  selectedId?: string | null;
  /** Pixels at the bottom of the canvas hidden behind the page's bottom sheet. */
  insetBottom?: number;
  /** Called with the card id on click, tap, Enter or Space. */
  onNodeSelect?: (id: string) => void;
  /** Accessible hint appended to every card, e.g. "Enter opens". */
  selectHint?: string;
};

type QommNodeData = {
  graphNode: GraphNode;
  selected: boolean;
  selectHint: string;
  onSelect?: (id: string) => void;
};

type QommEdgeData = {
  path: string;
  state: GraphEdge['state'];
  color: GraphEdge['color'];
  own: boolean;
  particle: boolean;
  reducedMotion: boolean;
};

type LabelData = { text: string; strong: boolean };

const QommNode = memo(({ data }: NodeProps<Node<QommNodeData>>) => {
  const item = data.graphNode;
  const interactive = typeof data.onSelect === 'function';
  const classes = [
    'qrf-node',
    `qrf-${item.type}`,
    item.badge ? 'has-badge' : '',
    interactive ? 'is-interactive' : '',
    data.selected ? 'is-selected' : '',
    ...(item.classes ?? []),
  ]
    .filter(Boolean)
    .join(' ');
  const name = [item.title, item.sub, item.badge].filter(Boolean).join(' — ');
  const label = interactive && data.selectHint ? `${name}. ${data.selectHint}` : name;
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (!interactive) return;
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      event.stopPropagation();
      data.onSelect?.(item.id);
    }
  };
  return (
    <article
      className={classes}
      aria-label={label}
      data-node-id={item.id}
      role={interactive ? 'button' : undefined}
      tabIndex={interactive ? 0 : undefined}
      aria-expanded={interactive ? data.selected : undefined}
      onKeyDown={onKeyDown}
    >
      <Handle type="target" position={Position.Top} className="qrf-handle" />
      <Handle type="target" position={Position.Left} id="left-in" className="qrf-handle" />
      <header className="qrf-node-header">
        <span className="qrf-node-kind">{item.title}</span>
        {item.badge ? <span className="qrf-badge">{item.badge}</span> : null}
      </header>
      {item.sub ? <div className="qrf-node-status">{item.sub}</div> : null}
      {item.metrics?.length ? (
        <dl className="qrf-metrics">
          {item.metrics.map((metric, index) => (
            <div
              className={`qrf-metric qrf-${metric.tone ?? 'normal'}`}
              key={`${metric.label}-${index}`}
              title={`${metric.fullLabel ?? metric.label}: ${metric.fullValue ?? metric.value}`}
              aria-label={`${metric.fullLabel ?? metric.label}: ${metric.fullValue ?? metric.value}`}
            >
              <dt>{metric.label}</dt>
              <dd>{metric.value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
      <Handle type="source" position={Position.Bottom} className="qrf-handle" />
      <Handle type="source" position={Position.Right} id="right-out" className="qrf-handle" />
    </article>
  );
});
QommNode.displayName = 'QommNode';

const FlowLabel = memo(({ data }: NodeProps<Node<LabelData>>) => (
  <div className={`qrf-edge-label${data.strong ? ' strong' : ''}`}>{data.text}</div>
));
FlowLabel.displayName = 'FlowLabel';

const TransactionEdge = memo((props: EdgeProps<Edge<QommEdgeData>>) => {
  const { id, data, markerEnd } = props;
  if (!data) return null;
  const classes = [
    'qrf-edge',
    `qrf-edge-${data.state}`,
    `qrf-edge-${data.color}`,
    data.own ? 'qrf-edge-owned' : 'qrf-edge-faint',
  ].join(' ');
  return (
    <>
      <BaseEdge id={id} path={data.path} markerEnd={markerEnd} className={classes} />
      {data.state === 'flow' && data.own && data.particle && !data.reducedMotion ? (
        <circle r="5" className={`qrf-particle qrf-particle-${data.color}`}>
          <animateMotion dur="1.35s" repeatCount="indefinite" path={data.path} />
        </circle>
      ) : null}
    </>
  );
});
TransactionEdge.displayName = 'TransactionEdge';

const nodeTypes = { qommNode: QommNode, flowLabel: FlowLabel };
const edgeTypes = { transaction: TransactionEdge };

const MIN_ZOOM = 0.18;
const MAX_ZOOM = 1.8;
const FIT_OPTIONS = { padding: 0.06, maxZoom: 1, duration: 0 };
const COMPACT_PAD = 12;

function canvasElement(): HTMLElement | null {
  return document.querySelector<HTMLElement>('#network-graph .qrf-canvas');
}

// Use current geometry and the actual overlay bounds in one update. This
// prevents a size-change fit from hiding the selected card under a sheet.
function ViewportPolicy({ model, compact, selectedId, insetBottom }: { model: QommGraphModel; compact: boolean; selectedId: string | null; insetBottom: number }) {
  const { setViewport } = useReactFlow();
  const geometry = model.nodes.map(n => [n.id, n.x, n.y, n.w, n.h].join(":")).join(";");
  const apply = useRef<() => void>(() => {});
  apply.current = () => {
    const canvas = canvasElement();
    if (!canvas || !model.nodes.length) return;
    const rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const status = document.getElementById('stage-status')?.getBoundingClientRect();
    const legend = document.getElementById('graph-legend')?.getBoundingClientRect();
    const topInset = Math.max(COMPACT_PAD, status?.height ? status.bottom - rect.top + COMPACT_PAD : 0);
    const bottomInset = Math.max(insetBottom, legend?.height ? rect.bottom - legend.top + COMPACT_PAD : COMPACT_PAD);
    const usableHeight = Math.max(100, rect.height - topInset - bottomInset);
    const left = Math.min(...model.nodes.map(n => n.x - n.w / 2));
    const top = Math.min(...model.nodes.map(n => n.y - n.h / 2));
    const right = Math.max(...model.nodes.map(n => n.x + n.w / 2));
    const bottom = Math.max(...model.nodes.map(n => n.y + n.h / 2));
    const viewport = getViewportForBounds({ x: left, y: top, width: right - left, height: bottom - top }, rect.width, usableHeight, MIN_ZOOM, 1, 0.06);
    viewport.y += topInset;
    if (!document.body.classList.contains('details-hidden') && !compact && viewport.zoom < 1.5) {
      // Keep card text within the requested 1.5–2x enlargement; do not shrink to fit
      // the status overlay and drawer. The existing canvas can be panned.
      viewport.zoom = 1.5;
      viewport.x = (rect.width - (right - left) * viewport.zoom) / 2 - left * viewport.zoom;
      const selected = selectedId ? model.nodes.find(n => n.id === selectedId) : null;
      if (selected) viewport.x = rect.width / 2 - selected.x * viewport.zoom;
      viewport.y = selected
        ? topInset + Math.max(usableHeight / 2, selected.h * viewport.zoom / 2) - selected.y * viewport.zoom
        : topInset - top * viewport.zoom;
    }
    if (compact && !document.body.classList.contains('details-hidden')) {
      viewport.zoom = 1.5;
      viewport.x = (rect.width - model.W * viewport.zoom) / 2;
      const selected = insetBottom && selectedId ? model.nodes.find(n => n.id === selectedId) : null;
      if (selected) viewport.x = rect.width / 2 - selected.x * viewport.zoom;
      viewport.y = selected
        ? topInset + Math.max(usableHeight / 2, selected.h * viewport.zoom / 2) - selected.y * viewport.zoom
        : topInset - top * viewport.zoom;
    }
    void setViewport(viewport, { duration: 0 });
  };
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => apply.current());
    return () => window.cancelAnimationFrame(frame);
  }, [geometry, compact, selectedId, insetBottom]);
  useEffect(() => {
    if (typeof ResizeObserver === 'undefined') return;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => apply.current());
    });
    [canvasElement(), document.getElementById('stage-status'), document.getElementById('graph-legend')].forEach(element => { if (element) observer.observe(element); });
    return () => { observer.disconnect(); window.cancelAnimationFrame(frame); };
  }, []);
  return null;
}

function FlowCanvas({ model, options }: { model: QommGraphModel; options: QommGraphOptions }) {
  const compact = Boolean(options.compact);
  const selectedId = options.selectedId ?? null;
  const onSelect = options.onNodeSelect;
  const selectHint = options.selectHint ?? '';

  const nodes = useMemo<Node[]>(() => {
    const serviceNodes = model.nodes.map((item) => ({
      id: item.id,
      type: 'qommNode',
      width: item.w,
      height: item.h,
      initialWidth: item.w,
      initialHeight: item.h,
      position: { x: item.x - item.w / 2, y: item.y - item.h / 2 },
      style: { width: item.w, height: item.h, minHeight: item.h },
      data: { graphNode: item, selected: item.id === selectedId, selectHint, onSelect },
      draggable: false,
      selectable: false,
      focusable: false,
    }));
    const labels = model.labels.map((label, index) => ({
      id: `flow-label-${index}`,
      type: 'flowLabel',
      position: { x: label.x - 90, y: label.y - 12 },
      style: { width: 180 },
      data: { text: label.text, strong: Boolean(label.strong) },
      draggable: false,
      selectable: false,
      focusable: false,
      connectable: false,
    }));
    return [...serviceNodes, ...labels];
  }, [model, selectedId, selectHint, onSelect]);

  const edges = useMemo<Edge[]>(() => model.edges.map((item) => ({
    id: item.id,
    type: 'transaction',
    source: item.source,
    target: item.target,
    animated: item.state === 'flow' && !options.reducedMotion,
    markerEnd: item.state === 'cut' ? undefined : {
      type: MarkerType.ArrowClosed,
      color: item.state === 'flow'
        ? (item.color === 'teal' ? '#45d7c8' : item.color === 'amber' ? '#f5b942' : '#69a7ff')
        : '#637089',
      width: 16,
      height: 16,
    },
    data: {
      path: item.d,
      state: item.state,
      color: item.color,
      own: item.own,
      particle: Boolean(item.particle),
      reducedMotion: options.reducedMotion,
    },
  })), [model, options.reducedMotion]);

  useEffect(() => {
    const live = document.querySelector('#network-graph .react-flow__viewport');
    live?.setAttribute('aria-live', 'polite');
  }, [options.phase]);

  // A click on a card opens its drawer. A drag that started on a card pans the
  // canvas and d3-zoom suppresses the click that would follow it, so panning
  // never opens a drawer by accident.
  const onNodeClick = useCallback<NodeMouseHandler>((_event, node) => {
    if (node.type === 'qommNode') onSelect?.(node.id);
  }, [onSelect]);

  return (
    <div className="qrf-shell">
      <div className="qrf-canvas">
        <ReactFlow
          aria-label={options.ariaLabel}
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          minZoom={MIN_ZOOM}
          maxZoom={MAX_ZOOM}
          fitView={false}
          fitViewOptions={FIT_OPTIONS}
          nodesConnectable={false}
          nodesDraggable={false}
          nodesFocusable={false}
          edgesFocusable={false}
          elementsSelectable={false}
          panOnDrag
          zoomOnDoubleClick={false}
          onNodeClick={onNodeClick}
          proOptions={{ hideAttribution: false }}
        >
          <Background variant={BackgroundVariant.Dots} gap={22} size={1.3} color="#26354b" />
          <Controls showInteractive={false} position={compact ? 'top-right' : 'bottom-right'} />
          <ViewportPolicy model={model} compact={compact} selectedId={selectedId} insetBottom={options.insetBottom ?? 0} />
        </ReactFlow>
      </div>
    </div>
  );
}

const roots = new WeakMap<HTMLElement, Root>();

// The container's size is the page's decision (it fills the viewport under
// the top bar); the bundle no longer sets a document height for it.
function render(container: HTMLElement, model: QommGraphModel, options: QommGraphOptions) {
  let root = roots.get(container);
  if (!root) {
    root = createRoot(container);
    roots.set(container, root);
  }
  root.render(
    <ReactFlowProvider>
      <FlowCanvas model={model} options={options} />
    </ReactFlowProvider>,
  );
}

declare global {
  interface Window {
    QommNetworkGraph?: { render: typeof render };
  }
}

window.QommNetworkGraph = { render };
