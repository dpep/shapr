class Encoder
  def encode(params)
    params.join("&")
  rescue TypeError
    ""
  else
    "none"
  end
end
