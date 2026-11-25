# typed: true
class A
  extend T::Sig

  def reset
    T.cast(self, A, checked: false).count = 0
    T.unsafe(T.let(self, A, checked: false)).total += 1
  end

  private

  attr_accessor :count, :total
end
