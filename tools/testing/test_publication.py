"""Do not publish inaccurate counts, dirty-source results or false green gates."""
import copy
import subprocess
import unittest
from harness import ROOT
from publish import checked


class PublicationChecks(unittest.TestCase):
    def report(self):
        commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
        return {'origin':'local','source_dirty':False,'source_commit':commit,'status':'passed','groups':[{'id':'contracts','status':'passed','exit_code':0,'tests':2,'passed':2,'failed':0,'skipped':0}],'counts':{'tests':2,'passed':2,'failed':0,'skipped':0}}

    def test_failing_gate_cannot_claim_pass(self):
        report=self.report();report['groups'][0]['exit_code']=1
        with self.assertRaises(ValueError):checked(report)

    def test_github_origin_requires_a_real_workflow_url(self):
        report=self.report();report['origin']='github_actions';report['run_url']='https://example.net/not-evidence'
        with self.assertRaises(ValueError):checked(report)

    def test_dirty_result_cannot_be_published(self):
        report=self.report();report['source_dirty']=True
        with self.assertRaises(ValueError):checked(report)

    def test_skipped_cases_cannot_be_counted_as_passed(self):
        report=self.report();report['groups'][0]['skipped']=1
        with self.assertRaises(ValueError):checked(report)

    def test_inflated_totals_cannot_be_published(self):
        report=self.report();report['counts']['passed']=999
        with self.assertRaises(ValueError):checked(report)

    def test_honest_failure_can_be_published_as_failure(self):
        report=copy.deepcopy(self.report());report['status']='failed';report['groups'][0].update(status='failed',exit_code=1,failed=1,passed=1);report['counts'].update(passed=1,failed=1)
        self.assertEqual(checked(report)['status'],'failed')
