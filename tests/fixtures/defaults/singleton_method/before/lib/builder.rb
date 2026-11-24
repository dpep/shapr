class Builder
  def self.build(x, strict: false)
    [x, strict]
  end

  def self.make(x)
    build(x, strict: false)
  end
end
