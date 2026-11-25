module Mac
  PATTERN = /\A\h{1,2}(:\h{1,2}){5}\z/

  def self.match?(text)
    PATTERN.match?(text)
  end
end
