"""A local snapshot may link only complete, successful CI for its exact source."""
import unittest
from record_ci import REQUIRED,proof

class CiEvidence(unittest.TestCase):
    def fixture(self):
        return {'id':42,'name':'Beta release','run_attempt':1,'status':'completed','conclusion':'success','head_sha':'a'*40,'repository':{'full_name':'RIRRd/RIRRd'},'html_url':'https://github.com/RIRRd/RIRRd/actions/runs/42'},[{'name':'build-suite / '+name,'status':'completed','conclusion':'success'}for name in sorted(REQUIRED)]
    def test_success_keeps_exact_source_and_real_job_names(self):
        run,jobs=self.fixture();result=proof(run,jobs,'a'*40,'RIRRd/RIRRd')
        self.assertEqual(result['source_commit'],'a'*40);self.assertEqual(len(result['jobs']),7)
    def test_failed_or_incomplete_workflow_is_rejected(self):
        for change in [{'status':'in_progress'},{'conclusion':'failure'}]:
            run,jobs=self.fixture();run.update(change)
            with self.assertRaises(ValueError):proof(run,jobs,'a'*40,'RIRRd/RIRRd')
    def test_other_source_or_repository_is_rejected(self):
        for source,repo in [('b'*40,'RIRRd/RIRRd'),('a'*40,'other/project')]:
            run,jobs=self.fixture()
            with self.assertRaises(ValueError):proof(run,jobs,source,repo)
    def test_missing_failed_and_cancelled_jobs_are_rejected(self):
        run,jobs=self.fixture()
        for variant in [jobs[:-1],jobs+[{'name':'packages','status':'completed','conclusion':'failure'}],jobs+[{'name':'packages','status':'completed','conclusion':'cancelled'}]]:
            with self.assertRaises(ValueError):proof(run,variant,'a'*40,'RIRRd/RIRRd')
