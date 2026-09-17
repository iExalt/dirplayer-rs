import { useEffect, useRef, useState } from 'react';
import styles from './styles.module.css';
import { useVMHandle } from '../../components/VMProvider';
import { useAppSelector, useAppDispatch } from '../../store/hooks';
import { DebugMessage, DebugMessageDatum, debugMessagesCleared, selectDebugMessages } from '../../store/vmSlice';
import { DatumRef, ScriptInstanceId, TJsBridgeDatumList, TJsBridgeDatumScriptInstance } from '../../vm';

const PAGE_SIZE = 20;

function BitmapCanvas({ width, height, data }: { width: number; height: number; data: Uint8Array }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const imageData = new ImageData(
      new Uint8ClampedArray(data.buffer as ArrayBuffer, data.byteOffset, data.byteLength),
      width,
      height,
    );
    ctx.putImageData(imageData, 0, 0);
  }, [width, height, data]);

  return <canvas ref={canvasRef} className={styles.bitmapCanvas} />;
}

function DatumValueDisplay({ datumRef }: { datumRef: DatumRef }) {
  const handle = useVMHandle();
  const snapshot = useAppSelector((state) => {
    if (datumRef === 0) return { type: 'void' as const, debugDescription: '<Void>' };
    return state.vm.datumSnapshots[datumRef];
  });
  const [isExpanded, setIsExpanded] = useState(false);

  useEffect(() => {
    if (!snapshot && datumRef !== 0) {
      void handle.request_datum(datumRef);
    }
  }, [handle, snapshot, datumRef]);

  if (!snapshot) {
    return <span className={styles.datumLoading}>Loading...</span>;
  }

  const isExpandable = snapshot.type === 'list' || snapshot.type === 'propList' || snapshot.type === 'scriptInstance';

  return (
    <span>
      <span
        className={isExpandable ? styles.datumExpandable : undefined}
        onClick={isExpandable ? () => setIsExpanded(!isExpanded) : undefined}
      >
        {isExpandable && <span className={styles.datumToggle}>{isExpanded ? '\u25BC' : '\u25B6'} </span>}
        {snapshot.debugDescription}
      </span>
      {isExpanded && snapshot.type === 'list' && (
        <DatumListItems datum={snapshot} />
      )}
      {isExpanded && snapshot.type === 'propList' && (
        <DatumPropListItems datum={snapshot} />
      )}
      {isExpanded && snapshot.type === 'scriptInstance' && (
        <ScriptInstanceItems datum={snapshot} />
      )}
    </span>
  );
}

function ScriptInstanceDisplay({ instanceId }: { instanceId: ScriptInstanceId }) {
  const handle = useVMHandle();
  const snapshot = useAppSelector((state) => state.vm.scriptInstanceSnapshots[instanceId]);
  const [isExpanded, setIsExpanded] = useState(false);

  useEffect(() => {
    if (!snapshot) {
      void handle.request_script_instance_snapshot(instanceId);
    }
  }, [handle, snapshot, instanceId]);

  if (!snapshot) {
    return <span className={styles.datumLoading}>Loading...</span>;
  }

  const isExpandable = snapshot.type === 'scriptInstance';

  return (
    <span>
      <span
        className={isExpandable ? styles.datumExpandable : undefined}
        onClick={isExpandable ? () => setIsExpanded(!isExpanded) : undefined}
      >
        {isExpandable && <span className={styles.datumToggle}>{isExpanded ? '\u25BC' : '\u25B6'} </span>}
        {snapshot.debugDescription}
      </span>
      {isExpanded && snapshot.type === 'scriptInstance' && (
        <ScriptInstanceItems datum={snapshot} />
      )}
    </span>
  );
}

function DatumListItems({ datum }: { datum: TJsBridgeDatumList }) {
  const [page, setPage] = useState(0);
  const items = datum.items;
  const totalItems = items.length;
  const totalPages = Math.ceil(totalItems / PAGE_SIZE);
  const start = page * PAGE_SIZE;
  const end = Math.min(start + PAGE_SIZE, totalItems);
  const pageItems = items.slice(start, end);

  return (
    <div className={styles.datumChildren}>
      {pageItems.map((itemRef, i) => (
        <div key={start + i} className={styles.datumEntry}>
          <span className={styles.datumKey}>[{start + i + 1}]:</span>{' '}
          <DatumValueDisplay datumRef={itemRef} />
        </div>
      ))}
      {totalPages > 1 && (
        <div className={styles.pagination}>
          <button disabled={page === 0} onClick={() => setPage(page - 1)}>&laquo; Prev</button>
          <span>{start + 1}&ndash;{end} of {totalItems}</span>
          <button disabled={page >= totalPages - 1} onClick={() => setPage(page + 1)}>Next &raquo;</button>
        </div>
      )}
    </div>
  );
}

function DatumPropListItems({ datum }: { datum: { properties: Record<string, DatumRef> } }) {
  const entries = Object.entries(datum.properties);
  const [page, setPage] = useState(0);
  const totalItems = entries.length;
  const totalPages = Math.ceil(totalItems / PAGE_SIZE);
  const start = page * PAGE_SIZE;
  const end = Math.min(start + PAGE_SIZE, totalItems);
  const pageEntries = entries.slice(start, end);

  return (
    <div className={styles.datumChildren}>
      {pageEntries.map(([key, valueRef]) => (
        <div key={key} className={styles.datumEntry}>
          <span className={styles.datumKey}>{key}:</span>{' '}
          <DatumValueDisplay datumRef={valueRef} />
        </div>
      ))}
      {totalPages > 1 && (
        <div className={styles.pagination}>
          <button disabled={page === 0} onClick={() => setPage(page - 1)}>&laquo; Prev</button>
          <span>{start + 1}&ndash;{end} of {totalItems}</span>
          <button disabled={page >= totalPages - 1} onClick={() => setPage(page + 1)}>Next &raquo;</button>
        </div>
      )}
    </div>
  );
}

function ScriptInstanceItems({ datum }: { datum: TJsBridgeDatumScriptInstance }) {
  return (
    <div className={styles.datumChildren}>
      {datum.ancestor != null && (
        <div className={styles.datumEntry}>
          <span className={styles.datumKey}>ancestor:</span>{' '}
          <ScriptInstanceDisplay instanceId={datum.ancestor} />
        </div>
      )}
      <DatumPropListItems datum={datum} />
    </div>
  );
}

function DatumInspector({ message }: { message: DebugMessageDatum }) {
  const { snapshot } = message;
  const [isExpanded, setIsExpanded] = useState(true);

  return (
    <div className={styles.datumInspector}>
      <span
        className={styles.datumExpandable}
        onClick={() => setIsExpanded(!isExpanded)}
      >
        <span className={styles.datumToggle}>{isExpanded ? '\u25BC' : '\u25B6'} </span>
        {snapshot.debugDescription}
      </span>
      {isExpanded && snapshot.type === 'list' && (
        <DatumListItems datum={snapshot} />
      )}
      {isExpanded && snapshot.type === 'propList' && (
        <DatumPropListItems datum={snapshot} />
      )}
      {isExpanded && snapshot.type === 'scriptInstance' && (
        <ScriptInstanceItems datum={snapshot} />
      )}
    </div>
  );
}

function DebugMessageEntry({ message }: { message: DebugMessage }) {
  switch (message.type) {
    case 'text':
      return <span>{message.content}{'\n'}</span>;
    case 'bitmap':
      return <BitmapCanvas width={message.width} height={message.height} data={message.data} />;
    case 'datum':
      return <DatumInspector message={message} />;
  }
}

export default function MessageInspector() {
  const handle = useVMHandle();
  const [command, setCommand] = useState('');
  const [history, setHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const debugMessages = useAppSelector(({ vm }) => selectDebugMessages(vm));
  const messageLogRef = useRef<HTMLDivElement>(null);
  const dispatch = useAppDispatch();

  useEffect(() => {
    messageLogRef.current?.scrollTo(0, messageLogRef.current.scrollHeight);
  }, [debugMessages]);

  const handleClear = () => {
    void handle.clear_debug_messages();
    dispatch(debugMessagesCleared());
  };

  const handleEvaluate = () => {
    try {
      if (command.trim()) {
        void handle.eval_command(command);
        setHistory(prev => [...prev, command]);
        setHistoryIndex(-1);
      }
      setCommand('');
    } catch (err) {
      console.error(err);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      handleEvaluate();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (history.length > 0) {
        const newIndex = historyIndex === -1 ? history.length - 1 : Math.max(0, historyIndex - 1);
        setHistoryIndex(newIndex);
        setCommand(history[newIndex]);
      }
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (historyIndex !== -1) {
        const newIndex = Math.min(history.length - 1, historyIndex + 1);
        if (newIndex === history.length - 1 && historyIndex === history.length - 1) {
          setHistoryIndex(-1);
          setCommand('');
        } else {
          setHistoryIndex(newIndex);
          setCommand(history[newIndex]);
        }
      }
    }
  };

  return (
    <div className={styles.container}>
      <div ref={messageLogRef} className={styles.debugMessages}>
        {debugMessages.map((msg, i) => (
          <DebugMessageEntry key={i} message={msg} />
        ))}
      </div>
      <div className={styles.evalSection}>
        <div className={styles.inputGroup}>
          <input
            type="text"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Enter command to evaluate..."
            className={styles.commandInput}
          />
          <button onClick={handleEvaluate} className={styles.evalButton}>
            Evaluate
          </button>
          <button onClick={handleClear} className={styles.clearButton}>
            Clear
          </button>
        </div>
      </div>
    </div>
  )
}
