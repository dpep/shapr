class A
  module T
    def self.unsafe(x) = x.dup.freeze
  end

  def run(x)
    T.unsafe(x) << 1
  end
end
