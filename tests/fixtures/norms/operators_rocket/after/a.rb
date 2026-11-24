def headers(token)
  base = { "accept" => "json" }
  extra = base.merge("x" => 1)
  auth = { "token"=>token }
  sum = 1 +  2
  [base, extra, auth, sum]
end
