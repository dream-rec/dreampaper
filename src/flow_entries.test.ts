import { describe, expect, it } from 'vitest';
import { buildFlowEntries, copy, searchRounds, stageLabel, type DesignCard } from './app';

function card(step: string, label = step, status: DesignCard['status'] = 'succeeded'): DesignCard {
  return { step, label, text: `${step} text`, status };
}

describe('buildFlowEntries', () => {
  it('按执行顺序排列并标记独立阶段：查询、模型步骤、最终提示词', () => {
    const entries = buildFlowEntries(
      [card('paper_design', '生成制图方案'), card('paper_structure', '分析母版结构')],
      [{ index: 1, label: '联网查询 1 · Docker', request: 'req', response: 'res' }],
      [card('final_prompt_1', '最终制图提示词 1')],
      copy.zh,
      'paper_figure'
    );

    expect(entries.map((entry) => entry.key)).toEqual([
      'search-1',
      'paper_structure',
      'paper_design',
      'final_prompt_1'
    ]);
    expect(entries[0]).toMatchObject({ kind: 'search', request: 'req', response: 'res' });
    expect(entries[3].kind).toBe('prompt');
    expect(entries.map((entry) => entry.phase)).toEqual(['search', 'design', 'design', 'implement']);
  });

  it('比对历史案例排在分析母版与大纲规划之间', () => {
    const slides = buildFlowEntries(
      [card('ppt_outline', '规划整套大纲'), card('advisor', '比对历史案例'), card('ppt_analyze', '分析母版'), card('ppt_page_plan_1', '规划第 1 页内容')],
      [], [], copy.zh, 'ppt_slide'
    );
    expect(slides.map((entry) => entry.step)).toEqual(['ppt_analyze', 'advisor', 'ppt_outline', 'ppt_page_plan_1']);

    const figure = buildFlowEntries(
      [card('paper_design', '生成制图方案'), card('advisor', '比对历史案例'), card('paper_structure', '分析母版结构')],
      [], [], copy.zh, 'paper_figure'
    );
    expect(figure.map((entry) => entry.step)).toEqual(['paper_structure', 'advisor', 'paper_design']);
  });

  it('幻灯片按页号排提示词，并标成第 N 页', () => {
    const entries = buildFlowEntries(
      [card('ppt_page_plan_2', '规划第 2 页内容'), card('ppt_analyze', '分析母版'), card('ppt_page_plan_1', '规划第 1 页内容')],
      [],
      [card('final_prompt_2'), card('final_prompt_1')],
      copy.zh,
      'ppt_slide'
    );

    expect(entries.map((entry) => entry.key)).toEqual([
      'ppt_analyze',
      'ppt_page_plan_1',
      'ppt_page_plan_2',
      'final_prompt_1',
      'final_prompt_2'
    ]);
    expect(entries.filter((entry) => entry.kind === 'prompt').map((entry) => entry.title)).toEqual([
      '第 1 页',
      '第 2 页'
    ]);
  });

  it('视觉依据虽存于模型日志，仍属于 Search 而非 Design', () => {
    const entries = buildFlowEntries(
      [card('paper_visual_assets'), card('ppt_visual_assets'), card('paper_design')],
      [], [], copy.zh, 'paper_figure'
    );
    expect(entries.map((entry) => entry.phase)).toEqual(['search', 'search', 'design']);
  });

  it('未返回的检索显示进行中，失败的检索不能显示成功', () => {
    const searches = searchRounds([
      card('search_request_1'),
      card('search_request_2'),
      card('search_response_2', 'failed search', 'failed')
    ]);
    const entries = buildFlowEntries([], searches, [], copy.zh, 'paper_figure');
    expect(entries.map((entry) => entry.status)).toEqual(['running', 'failed']);
  });

  it('没有查询轮次时不留空行', () => {
    const entries = buildFlowEntries([card('paper_design')], [], [], copy.zh, 'paper_figure');
    expect(entries.map((entry) => entry.kind)).toEqual(['step']);
  });
});

describe('stageLabel', () => {
  it('把阶段代码说成人话，按页阶段带上页号', () => {
    expect(stageLabel('ppt_analyze', copy.zh)).toBe('分析母版');
    expect(stageLabel('paper_visual_assets', copy.zh)).toBe('查询视觉素材');
    expect(stageLabel('ppt_page_plan_3', copy.zh)).toBe('规划第 3 页');
    expect(stageLabel('ppt_implement_3', copy.en)).toBe('Render page 3');
  });

  it('未知阶段原样返回，空值返回空串', () => {
    expect(stageLabel('some_new_stage', copy.zh)).toBe('some_new_stage');
    expect(stageLabel(undefined, copy.zh)).toBe('');
  });

  it('中英文都覆盖同一批阶段代码', () => {
    expect(Object.keys(copy.en.result.stages).sort()).toEqual(Object.keys(copy.zh.result.stages).sort());
  });
});
