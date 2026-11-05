#!/usr/bin/env python3
"""Compare ordered query aliases against pinned IRRd 4.5.4 using QUERY fixtures.

The reference and candidate must define QUERY-GROUP, QUERY-EMPTY, QUERY-SINGLE
as in irrd.yaml. Lists are compared without removing duplicate answers.
"""
import argparse
from collections import Counter
import json
from pathlib import Path
from urllib.parse import urlencode

from compare_query_scope import tcp, http, canonical, object_text, values, endpoint


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference-tcp', type=endpoint, default=('127.0.0.1', 8043))
    parser.add_argument('--candidate-tcp', type=endpoint, default=('127.0.0.1', 9043))
    parser.add_argument('--reference-http', default='http://127.0.0.1:8000')
    parser.add_argument('--candidate-http', default='http://127.0.0.1:9080')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    checked = Counter()
    differences = []

    def equal(group, label, expected, actual):
        assert expected == actual, (group, label, expected, actual)
        checked[group] += 1

    def graph(document, error=False):
        replies=[]
        for base in [args.reference_http, args.candidate_http]:
            status, raw = http(base, '/graphql/', document)
            data=json.loads(raw)
            assert status in ([200,400] if error else [200]), (base,status,raw)
            if error:
                assert data.get('errors'), (base,document,data)
                replies.append(True)
            else:
                assert not data.get('errors'), (base,document,data)
                replies.append(canonical(data['data']))
        equal('graphql_errors' if error else 'graphql_results', document, *replies)

    queries=['!s-lc','!maut-num,AS64496','!mas-set,AS-DUPLICATE',
             '!gAS64496','!6AS64496','!iAS-DUPLICATE,1','!a4AS-DUPLICATE',
             '!a6AS-DUPLICATE','!aAS-DUPLICATE','!iAS-INDIRECT,1']
    scopes=[['QUERY-GROUP'],['QUERY-FIRST','QUERY-GROUP'],
            ['QUERY-GROUP','QUERY-FIRST','QUERY-GROUP'], ['QUERY-SINGLE']]
    for sources in scopes:
        selected=','.join(sources)
        for query in queries:
            replies=[tcp(address,sources,query) for address in [args.reference_tcp,args.candidate_tcp]]
            norm=object_text if query.startswith('!m') else values
            equal('tcp_results', selected+' '+query,
                  (replies[0][0],norm(replies[0][1])), (replies[1][0],norm(replies[1][1])))
            if query=='!s-lc':
                equal('selection_order',selected,replies[0][1].decode().strip(),selected)
            status,body=http(args.candidate_http,'/v1/whois/?'+urlencode({'q':query,'sources':selected}))
            equal('http_alias_selection',selected+' '+query,
                  (200 if replies[0][0]=='A' else 204,norm(replies[0][1])), (status,norm(body)))
        if sources!=['QUERY-EMPTY']:
            for query in ['-i origin AS64496','AS-DUPLICATE']:
                scoped='-s '+selected+' '+query
                replies=[http(base,'/v1/whois/?'+urlencode({'q':scoped})) for base in [args.reference_http,args.candidate_http]]
                equal('http_common_aliases',scoped,(replies[0][0],object_text(replies[0][1])),(replies[1][0],object_text(replies[1][1])))
        arg=',sources:'+json.dumps(sources)
        graph('{asnPrefixes(asns:[64496,64497]'+arg+'){asn prefixes}}')
        graph('{asSetPrefixes(setNames:["AS-DUPLICATE"]'+arg+'){rpslPk prefixes}}')
        graph('{recursiveSetMembers(setNames:["AS-DUPLICATE"]'+arg+'){rpslPk rootSource members}}')
        graph('{rpslObjects(rpslPk:["AS64496"]'+arg+'){rpslPk source}}')
    for query in queries:
        reference=tcp(args.reference_tcp,['QUERY-EMPTY'],query)
        candidate=tcp(args.candidate_tcp,['QUERY-EMPTY'],query)
        if query=='!s-lc':
            equal('selection_order','empty alias',reference,candidate)
        else:
            assert candidate[0] in ['C','D'] and not candidate[1], (query,candidate)
            status,body=http(args.candidate_http,'/v1/whois/?'+urlencode({'q':query,'sources':'QUERY-EMPTY'}))
            assert status==204 and not body, (query,status,body)
            checked['empty_alias_fail_closed']+=1
            if reference!=candidate:
                differences.append({'query':query,'scope':'Empty alias remains empty in RIRRd; upstream may omit its source predicate.',
                                    'reference_frame':reference[0],'candidate_frame':candidate[0]})
    graph('{databaseStatus(sources:["QUERY-GROUP","QUERY-SINGLE","QUERY-EMPTY","QUERY-FIRST"]){source source_type aliased_sources}}')
    for field in ['authoritative','scopefilterEnabled','localJournalKept']:
        # The pinned schema marks these non-null but omits them for alias rows.
        graph('{databaseStatus(sources:["QUERY-GROUP"]){source '+field+'}}',error=True)
    for document in ['{asnPrefixes(asns:[64496],sources:["QUERY-UNKNOWN"]){prefixes}}',
                     '{recursiveSetMembers(setNames:["AS-DUPLICATE"],rootSource:"QUERY-GROUP"){members}}']:
        graph(document,error=True)
    for query in ['!jQUERY-GROUP','!jQUERY-EMPTY']:
        replies=[tcp(address,['QUERY-FIRST'],query) for address in [args.reference_tcp,args.candidate_tcp]]
        equal('no_mirror_aliases',query,replies[0],replies[1])
    # Reuse identical full-class synthetic records for the five additional
    # indexed arguments. Every query goes through normal HTTP GraphQL.
    for arguments in [
        'members:["AS64496"]', 'members:["AS-DUPLICATE"]',
        'members:["AS64496","AS64498","AS64496"]',
        'role:["UNKNOWN"]',
        'origin:["AS64496"]', 'origin:["as64496"]',
        'origin:["AS64496","AS64497"]', 'origin:["AS64496","AS64496"]',
        'origin:["AS64496"],objectClass:["route6"]',
        'origin:["AS64496"],mntBy:["QUERY-MNT"]',
        'origin:["AS64496"],mntBy:["UNKNOWN"]',
        'origin:["AS64496"],rpslPk:["192.0.2.0/24AS64496"]',
        'origin:["AS64497"],rpslPk:["192.0.2.0/24AS64496"]',
        'origin:["INVALID"]', 'origin:["64496"]',
    ]:
        graph('{rpslObjects(sources:["QUERY-GROUP"],'+arguments+'){rpslPk source objectClass}}')
    graph({'query':'query($origin:[String!],$sources:[String!]){rpslObjects(origin:$origin,sources:$sources){rpslPk source objectClass}}',
           'variables':{'origin':['AS64496'],'sources':['QUERY-GROUP']}})
    graph('{rpslObjects(origin:[64496],sources:["QUERY-GROUP"]){rpslPk}}',error=True)
    for arguments in ['person:["LOOKUP PERSON"]','person:["lookup person"]',
                      'role:["LOOKUP ROLE"]','role:["lookup role"]']:
        graph('{rpslObjects(sources:["QUERY-LOOKUP"],'+arguments+'){rpslPk source objectClass}}')
    for field, value, key in [('person','Query Fixture','QUERY1-TEST'),('role','Query Fixture NOC','QNOC1-TEST')]:
        document='{rpslObjects(sources:["QUERY-GROUP"],'+field+':'+json.dumps([value])+'){rpslPk source}}'
        observed=[]
        for base in [args.reference_http,args.candidate_http]:
            status,raw=http(base,'/graphql/',document)
            assert status==200
            observed.append(canonical(json.loads(raw)['data']))
        expected={'rpslObjects':[{'rpslPk':key,'source':'QUERY-FIRST'},{'rpslPk':key,'source':'QUERY-SECOND'}]}
        equal('display_name_normalization',field,canonical(expected),observed[1])
        assert observed[0]=={'rpslObjects':[]},observed[0]
        differences.append({'query':document,'scope':'RIRRd normalizes indexed display names consistently; pinned IRRd uppercases lookup values but preserves display-name case in parsed JSON.',
                            'reference':observed[0],'candidate':observed[1]})
    ipv6_doc='{rpslObjects(sources:["QUERY-GROUP"],mpMembers:["2001:db8:ffff::/48"]){rpslPk source}}'
    expected={'rpslObjects':[{'rpslPk':'RS-QUERY','source':'QUERY-FIRST'}, {'rpslPk':'RS-QUERY','source':'QUERY-SECOND'}]}
    observed=[]
    for base in [args.reference_http,args.candidate_http]:
        status,raw=http(base,'/graphql/',ipv6_doc)
        assert status==200
        observed.append(canonical(json.loads(raw)['data']))
    equal('ipv6_lookup_normalization','candidate canonical IPv6 members',canonical(expected),observed[1])
    assert observed[0]=={'rpslObjects':[]}, observed[0]
    differences.append({'query':ipv6_doc,'scope':'RIRRd normalizes inverse IPv6 values consistently; pinned IRRd uppercases lookup values but stores IPv6 parsed JSON lowercase.',
                        'reference':observed[0], 'candidate':observed[1]})
    for document in [
        '{rpslObjects(sources:["QUERY-EMPTY"],rpslPk:["AS64496"]){rpslPk}}',
        '{rpslObjects(sources:["QUERY-EMPTY"],origin:["AS64496"]){rpslPk}}',
        '{asnPrefixes(asns:[64496],sources:["QUERY-EMPTY"]){prefixes}}',
        '{asSetPrefixes(setNames:["AS-DUPLICATE"],sources:["QUERY-EMPTY"]){prefixes}}',
        '{recursiveSetMembers(setNames:["AS-DUPLICATE"],sources:["QUERY-EMPTY"]){members}}',
    ]:
        status,raw=http(args.candidate_http,'/graphql/',document)
        result=json.loads(raw)
        assert status==200 and not result.get('errors'),result
        rows=next(iter(result['data'].values()))
        assert all(not row.get('prefixes') and not row.get('members') and not row.get('rpslPk') for row in rows), result
        checked['empty_alias_fail_closed']+=1
    document='{rpslObjects(sources:["QUERY-GROUP"],members:[]){rpslPk source objectClass}}'
    rows=[]
    for base in [args.reference_http,args.candidate_http]:
        status,raw=http(base,'/graphql/',document)
        assert status==200
        rows.append(json.loads(raw)['data']['rpslObjects'])
    assert len(rows[0])==54 and rows[1]==[],rows
    checked['empty_filter_fail_closed']+=1
    differences.append({'query':document,'scope':'An explicit empty filter matches nothing in RIRRd; pinned IRRd drops the filter.',
                        'reference_count':54,'candidate_count':0})
    document='{rpslObjects(sources:["QUERY-GROUP"],person:null,rpslPk:["QUERY1-TEST"]){rpslPk source}}'
    reference=json.loads(http(args.reference_http,'/graphql/',document)[1])
    candidate=json.loads(http(args.candidate_http,'/graphql/',document)[1])
    assert reference.get('errors') and not candidate.get('errors') and len(candidate['data']['rpslObjects'])==2,(reference,candidate)
    checked['null_filter_omission']+=1
    differences.append({'query':document,'scope':'A null indexed filter is omitted by RIRRd; pinned IRRd raises an internal resolver error.'})
    fields={'members','mpMembers','person','role','origin'}
    introspection='{__type(name:"Query"){fields{name args{name type{kind name ofType{kind name ofType{kind name}}}}}}}'
    types=[]
    for base in [args.reference_http,args.candidate_http]:
        status,raw=http(base,'/graphql/',introspection)
        assert status==200
        field=next(item for item in json.loads(raw)['data']['__type']['fields'] if item['name']=='rpslObjects')
        types.append({arg['name']:arg['type'] for arg in field['args'] if arg['name'] in fields})
    equal('graphql_argument_types','five indexed arguments',types[0],types[1])
    assert set(types[0])==fields
    report={'reference':'IRRd 4.5.4 / 8e8080dab744a348b7d0e6efcb53f403301dd075',
            'checked':dict(checked),'total':sum(checked.values()),
            'deliberate_differences':differences,
            'scope':'Synthetic query aliases, ordered physical-source expansion, source status and rejection; not full production dataset equivalence.'}
    if args.report:
        args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__=='__main__':
    main()
