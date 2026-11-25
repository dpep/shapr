class Encoder
  def encode(params)
    buffer = params.map(&:to_s).join("&")
    buffer.chop
  end
end
