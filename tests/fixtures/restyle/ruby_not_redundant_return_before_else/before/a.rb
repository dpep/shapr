class Encoder
  def encode(params)
    return params.join("&")
  rescue TypeError
    ""
  else
    "none"
  end
end
