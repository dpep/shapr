def headers(token)
  base = { "accept" => "json" }
  extra = base.merge("x" => 1)
  [base, extra]
end
