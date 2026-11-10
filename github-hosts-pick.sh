#!/bin/bash
# github-hosts-pick.sh —— 自动测速挑选当前可用的 GitHub IP，并生成 hosts 片段
# 用法:
#   bash github-hosts-pick.sh          # 只测试并打印结果
#   bash github-hosts-pick.sh write    # 测试并写入 ./github-hosts.txt
# 说明: 每个域名对候选 IP 做 N 次 TLS+HTTP 实测，按 (成功率, 握手耗时) 排序取最优。

TRIALS=${TRIALS:-6}
OUT="$(dirname "$0")/github-hosts.txt"

# 域名|候选IP列表
DATA='
github.com|20.27.177.113 20.175.192.147 140.82.121.3 140.82.121.4 140.82.112.3 140.82.112.4 140.82.113.3 140.82.113.4 140.82.114.3 140.82.114.4 140.82.116.4 20.26.156.215 4.208.26.197 4.228.31.150
api.github.com|20.205.243.168 140.82.112.6 140.82.113.6 140.82.116.6
raw.githubusercontent.com|185.199.108.133 185.199.109.133 185.199.110.133 185.199.111.133
objects.githubusercontent.com|185.199.108.133 185.199.109.133 185.199.110.133 185.199.111.133
avatars.githubusercontent.com|185.199.108.133 185.199.109.133 185.199.110.133 185.199.111.133
media.githubusercontent.com|185.199.108.133 185.199.109.133 185.199.110.133 185.199.111.133
camo.githubusercontent.com|185.199.108.133 185.199.109.133 185.199.110.133 185.199.111.133
user-images.githubusercontent.com|185.199.108.133 185.199.109.133 185.199.110.133 185.199.111.133
github.githubassets.com|185.199.108.215 185.199.109.215 185.199.110.215 185.199.111.215
codeload.github.com|20.205.243.165 140.82.112.9 140.82.113.9
github.io|185.199.108.153 185.199.109.153 185.199.110.153 185.199.111.153
'

# 每个域名用哪个路径探测（能证明真的通且返回正常）
probe_path() {
    case "$1" in
        raw.githubusercontent.com) echo "/git/git/master/README.md" ;;
        github.githubassets.com)   echo "/favicons/favicon.svg" ;;
        github.com|api.github.com) echo "/" ;;
        *)                         echo "/" ;;
    esac
}

# 内容校验：光看 HTTP 状态码会被假的 "OK" 服务器骗过去
content_ok() {
    local sni=$1 body=$2
    [ -z "$body" ] && return 1
    case "$sni" in
        github.com)
            printf '%s' "$body" | grep -q '<title>GitHub' ;;
        api.github.com)
            printf '%s' "$body" | grep -q '"resources"\|rate_limit\|"current_user_url"' ;;
        raw.githubusercontent.com)
            [ "${#body}" -gt 10 ] && ! printf '%s' "$body" | grep -q '404: Not Found' ;;
        github.githubassets.com)
            printf '%s' "$body" | grep -qi '<svg\|DOCTYPE html' ;;
        codeload.github.com)
            [ "${#body}" -gt 20 ] && ! printf '%s' "$body" | grep -q 'Not Found' ;;
        *)
            [ "${#body}" -gt 0 ] ;;
    esac
}

# 测一个 IP，输出 "成功数 总握手秒数"
probe_ip() {
    local sni=$1 ip=$2 path=$3 ok=0 sum=0 n=0
    for _ in $(seq 1 "$TRIALS"); do
        local out code t body
        # 必须连内容一起取：有些 IP 会返回 200 + 一个 "OK" 字符串冒充成功
        out=$(curl -s -w '\n%{http_code} %{time_appconnect}' \
              --connect-timeout 4 -m 10 \
              --resolve "$sni:443:$ip" "https://$sni$path" 2>/dev/null)
        code=$(printf '%s' "$out" | tail -1 | awk '{print $1}')
        t=$(printf '%s' "$out" | tail -1 | awk '{print $2}')
        body=$(printf '%s' "$out" | sed '$d')
        if [ -n "$code" ] && [ "$code" != "000" ] && content_ok "$sni" "$body"; then
            ok=$((ok + 1)); n=$((n + 1))
            sum=$(awk -v a="$sum" -v b="$t" 'BEGIN{print a+b}')
        fi
    done
    local avg=9.999
    [ "$n" -gt 0 ] && avg=$(awk -v a="$sum" -v b="$n" 'BEGIN{printf "%.3f", a/b}')
    echo "$ok $avg"
}

RESULT=""
printf "%-34s %-16s %-8s %s\n" "域名" "选中IP" "成功率" "握手"
printf -- '------------------------------------------------------------------------\n'

echo "$DATA" | while IFS='|' read -r domain ips; do
    [ -z "$domain" ] && continue
    path=$(probe_path "$domain")
    best_ip=""; best_ok=-1; best_t=9.999
    for ip in $ips; do
        # 跳过明显不可达的（先做一次快速 TCP 探测，省时间）
        nc -z -G 2 -w 2 "$ip" 443 >/dev/null 2>&1 || continue
        read -r ok t <<< "$(probe_ip "$domain" "$ip" "$path")"
        if [ "$ok" -gt "$best_ok" ] || { [ "$ok" -eq "$best_ok" ] && \
           [ "$(awk -v a="$t" -v b="$best_t" 'BEGIN{print (a<b)?1:0}')" = "1" ]; }; then
            best_ok=$ok; best_t=$t; best_ip=$ip
        fi
    done
    if [ -n "$best_ip" ] && [ "$best_ok" -gt 0 ]; then
        printf "%-34s %-16s %-8s %ss\n" "$domain" "$best_ip" "$best_ok/$TRIALS" "$best_t"
        echo "$best_ip $domain"
    else
        printf "%-34s %-16s %s\n" "$domain" "(无可用)" "0/$TRIALS"
    fi
done > /tmp/github_hosts_stage.txt

cat /tmp/github_hosts_stage.txt

# 提取出 "IP 域名" 行写入文件
if [ "$1" = "write" ]; then
    {
        echo "# GitHub hosts —— 由 github-hosts-pick.sh 于 $(date '+%Y-%m-%d %H:%M:%S') 生成"
        echo "# 用法: sudo 把下面内容追加到 /etc/hosts，然后 sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder"
        grep -E '^[0-9]+\.' /tmp/github_hosts_stage.txt
    } > "$OUT"
    echo
    echo "已写入: $OUT"
fi
